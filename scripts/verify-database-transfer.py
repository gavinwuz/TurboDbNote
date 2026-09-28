"""Exercise native transfers between two fresh loopback MySQL instances."""
import argparse
import concurrent.futures
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--mysql-bin', type=Path, required=True)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
binary = args.mysql_bin.resolve()
client = shutil.which('mysql.exe' if os.name == 'nt' else 'mysql')
if not client or Path(client).resolve().parent != binary:
    raise SystemExit('The installed mysql client must match the explicit test directory.')
flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0


class Fixture:
    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix='transfer-mysql-', dir=repo / 'target')).resolve()
        assert self.root.is_relative_to((repo / 'target').resolve())
        self.log = (self.root / 'server.log').open('wb')
        self.process = None
        try:
            server = binary / ('mysqld.exe' if os.name == 'nt' else 'mysqld')
            data = self.root / 'data'
            common = [str(server), '--no-defaults', '--basedir=' + str(binary.parent), '--datadir=' + str(data)]
            initialized = subprocess.run(common + ['--initialize-insecure'], stdout=self.log, stderr=self.log, timeout=90, creationflags=flags)
            if initialized.returncode:
                raise RuntimeError('Isolated initialization failed')
            with socket.socket() as listener:
                listener.bind(('127.0.0.1', 0))
                self.port = listener.getsockname()[1]
            self.process = subprocess.Popen(common + ['--bind-address=127.0.0.1', '--port=' + str(self.port), '--mysqlx=0', '--skip-log-bin', '--ssl=0'], stdout=self.log, stderr=self.log, creationflags=flags)
            for _ in range(150):
                if self.process.poll() is not None:
                    raise RuntimeError('Isolated server stopped')
                try:
                    actual = Path(self.sql('SELECT @@datadir;')).resolve()
                    if actual != data.resolve():
                        raise RuntimeError('Datadir does not match the isolated fixture')
                    break
                except subprocess.CalledProcessError:
                    time.sleep(.2)
            else:
                raise RuntimeError('Isolated startup timed out')
            self.sql('CREATE DATABASE import_alpha; CREATE DATABASE import_beta;')
        except BaseException:
            self.close()
            raise

    def sql(self, sql):
        result = subprocess.run([client, '--no-defaults', '--host=127.0.0.1', '--port=' + str(self.port), '--user=root', '--password=' + str(), '--batch', '--skip-column-names'], input=sql.encode(), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, creationflags=flags, timeout=10)
        if result.returncode:
            raise subprocess.CalledProcessError(result.returncode, 'isolated mysql')
        return result.stdout.decode().strip()

    def close(self):
        if self.process is not None and self.process.poll() is None:
            if os.name == 'nt':
                # Windows mysqld launches a monitor child; stop the owned process tree.
                subprocess.run(['taskkill', '/PID', str(self.process.pid), '/T', '/F'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=flags, timeout=15)
            else:
                self.process.terminate()
            self.process.wait(timeout=15)
        self.log.close()


fixtures = []
try:
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
        futures = [executor.submit(Fixture) for _ in range(2)]
        failures = []
        for future in futures:
            try:
                fixtures.append(future.result())
            except Exception as error:
                failures.append(error)
        if failures:
            raise failures[0]
    print('Two isolated fixtures ready', flush=True)
    env = os.environ.copy()
    env['TRANSFER_SOURCE_PORT'] = str(fixtures[0].port)
    env['TRANSFER_TARGET_PORT'] = str(fixtures[1].port)
    result = subprocess.run(['cargo', 'test', '--locked', '-j', '1', '-p', 'turbodbn-services', 'mysql_native_file_export_import_and_cross_connection_copy', '--', '--ignored', '--test-threads=1'], cwd=repo, env=env)
    raise SystemExit(result.returncode)
finally:
    for fixture in fixtures:
        fixture.close()
