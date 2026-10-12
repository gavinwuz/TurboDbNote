"""Isolated Elasticsearch/TDengine HTTP contract and lifecycle checks."""
import argparse
import os
import http.server
import json
import pathlib
import queue
import subprocess
import threading

ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--runtime", type=pathlib.Path, default=ROOT / "target/preview/drivers/native")
runtime = parser.parse_args().runtime.resolve()


def read_line(process):
    result = queue.Queue(maxsize=1)
    threading.Thread(target=lambda: result.put(process.stdout.readline()), daemon=True).start()
    line = result.get(timeout=30)
    return json.loads(line)


def connect(driver, port, database):
    command = [str(runtime / ("turbodbnote-driver-host.exe" if os.name == "nt" else "turbodbnote-driver-host"))]
    process = subprocess.Popen(command, cwd=runtime, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    profile = dict(driver_id=driver, kind="External", host="127.0.0.1", port=port, user="", database=database, timeout_seconds=10, tls="Disabled", list_all_databases=False)
    process.stdin.write((json.dumps(dict(profile=profile, password="")) + "\n").encode()); process.stdin.flush()
    reply = read_line(process)
    if "info" not in reply:
        process.terminate(); process.wait(timeout=5)
        raise AssertionError("Driver handshake failed: " + reply.get("error", "protocol"))
    return process


def operation(process, database, action):
    request = dict(id=1, kind={"Workbench": dict(database=database, action=action)})
    process.stdin.write((json.dumps(request) + "\n").encode()); process.stdin.flush()
    return read_line(process)


def close(process):
    process.stdin.close()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.terminate(); process.wait(timeout=5)
        raise AssertionError("Driver did not release on stdin close")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path.endswith('/_count'):
            self.respond({'count': 3})
            return
        self.respond({"version": {"number": "fixture"}})

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path.endswith("/_search"):
            request = json.loads(data)
            assert request["size"] == 3 and request["track_total_hits"] is False
            content = 'x' * 65537 if self.path.startswith('/oversized/') else 'fixture'
            self.respond({"hits": {"hits": [{"_id": str(i), "_source": {"name": content}} for i in range(3)]}})
        else:
            sql = data.decode()
            if "SERVER_VERSION" in sql:
                columns, rows = [["version", "VARCHAR", 20]], [["fixture"]]
            elif sql.startswith("DESCRIBE "):
                columns, rows = [["field", "VARCHAR", 20], ["type", "VARCHAR", 20], ["length", "INT", 4], ["note", "VARCHAR", 20]], [["ts", "TIMESTAMP", 8, ""]]
            elif sql == "SHOW TABLES":
                columns, rows = [["table_name", "VARCHAR", 20]], [["meters"]]
            else:
                columns, rows = [["ts", "TIMESTAMP", 8]], [["2026-01-01"]]
            self.respond({"code": 0, "column_meta": columns, "data": rows})

    def respond(self, data):
        payload = json.dumps(data).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(payload))); self.end_headers(); self.wfile.write(payload)


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    for driver, database in [("elasticsearch", "fixture"), ("tdengine", "fixture")]:
        process = connect(driver, server.server_port, database)
        try:
            objects = operation(process, database, "Catalog")["Ok"]["Workbench"]["Catalog"]["objects"]
            result = operation(process, database, {"GridPage": {"object": objects[0], "options": {"page_size": 2, "offset": 0}}})
            assert result["Ok"]["Workbench"]["GridPage"]["rows"]["values"]
            assert "Err" in operation(process, database, {"Execute": {"sql": "DROP TABLE fixture", "guard": None}})
            if driver == 'elasticsearch':
                large = dict(objects[0], name='oversized')
                assert operation(process, database, {'GridPage': {'object': large, 'options': {'page_size': 2, 'offset': 0}}}) == {'Err': 'drivers.value_limit'}
                assert operation(process, database, {'Count': {'object': objects[0], 'filter': None}})['Ok']['Workbench']['Count'] == 3
        finally:
            close(process)
        print(driver, "bounded catalog/page, rejected writes, clean exit: PASS")
finally:
    server.shutdown(); server.server_close()

# Failures must not echo submitted credentials, SQL or malformed input.
for payload in [b'x' * 65538, json.dumps({'profile': {'driver_id': 'unknown'}, 'password': 'fixture-secret-never-echo'}).encode()]:
    check = subprocess.run([str(runtime / 'turbodbnote-driver-host.exe')], cwd=runtime, input=payload+b'\n', stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    reply = json.loads(check.stdout)
    assert reply.get('error') in ('drivers.invalid_identity', 'drivers.value_limit')
    assert b'fixture-secret-never-echo' not in check.stdout + check.stderr
    assert not check.stderr
print('Rust worker: malformed/oversized handshake rejected, credentials not echoed: PASS')
