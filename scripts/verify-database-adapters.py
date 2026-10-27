"""Run only isolated localhost fixtures. Never use existing database data directories."""
import argparse,json,socket,subprocess,tempfile,threading,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs,urlparse
parser=argparse.ArgumentParser();parser.add_argument('--redis-server');parser.add_argument('--postgres-bin');parser.add_argument('--http-contract',action='store_true');args=parser.parse_args()
root=Path(__file__).resolve().parents[1]
probe=root/'target/debug/examples/database_probe.exe'
assert probe.is_file(),'Build: cargo build -p turbodbn-services --example database_probe'
def free_port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def wait_server(process,port):
 deadline=time.monotonic()+30
 while time.monotonic()<deadline:
  assert process.poll() is None,'Fixture exited'
  try:
   with socket.create_connection(('127.0.0.1',port),timeout=.2):return
  except OSError:time.sleep(.1)
 raise AssertionError('Fixture did not start')
def run_probe(kind,port,db):subprocess.run([str(probe),kind,str(port),db],cwd=root,check=True,timeout=45)
if args.redis_server:
 with tempfile.TemporaryDirectory(prefix='redis-fixture-',dir=root/'target') as folder:
  server_path=Path(args.redis_server).resolve();port=free_port()
  server=subprocess.Popen([str(server_path),'--bind','127.0.0.1','--port',str(port),'--dir',folder,'--save','','--appendonly','no'],creationflags=subprocess.CREATE_NO_WINDOW,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   wait_server(server,port)
   subprocess.run([str(server_path.with_name('redis-cli.exe')),'-h','127.0.0.1','-p',str(port),'SET','fixture:key','value'],check=True,capture_output=True)
   run_probe('redis',port,'0')
  finally:
   server.terminate();server.wait(timeout=10)
if args.postgres_bin:
 with tempfile.TemporaryDirectory(prefix='pg-fixture-',dir=root/'target') as folder:
  tools=Path(args.postgres_bin).resolve();data=Path(folder)/'data';port=free_port()
  subprocess.run([str(tools/'initdb.exe'),'-D',str(data),'-U','postgres','-A','trust','--encoding=UTF8','--no-locale'],check=True,capture_output=True,timeout=90)
  server=subprocess.Popen([str(tools/'postgres.exe'),'-D',str(data),'-h','127.0.0.1','-p',str(port),'-c','ssl=off'],creationflags=subprocess.CREATE_NO_WINDOW,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   wait_server(server,port)
   deadline=time.monotonic()+30
   while True:
    ready=subprocess.run([str(tools/'pg_isready.exe'),'-h','127.0.0.1','-p',str(port),'-U','postgres','-d','postgres'],capture_output=True,timeout=5)
    if ready.returncode==0:break
    assert time.monotonic()<deadline,ready.stderr.decode(errors='replace')
    time.sleep(.1)
   subprocess.run([str(tools/'psql.exe'),'-h','127.0.0.1','-p',str(port),'-U','postgres','-d','postgres','-c','CREATE TABLE fixture_items(id integer primary key,name text); CREATE INDEX fixture_name ON fixture_items(name);'],check=True,capture_output=True)
   run_probe('pg',port,'postgres')
  finally:
   subprocess.run([str(tools/'pg_ctl.exe'),'-D',str(data),'-m','fast','stop'],check=False,capture_output=True,timeout=30)
   server.wait(timeout=10)
if args.http_contract:
 seen=[]
 class Handler(BaseHTTPRequestHandler):
  def log_message(self,*args):pass
  def do_POST(self):
   sql=self.rfile.read(int(self.headers['Content-Length'])).decode();params=parse_qs(urlparse(self.path).query);seen.append((sql,params))
   assert self.headers.get('Authorization','').startswith('Basic ')
   if sql.endswith('FORMAT JSONCompactEachRow') or params.get('default_format') in [['JSON'],['JSONCompact']]:
    assert params.get('database')==['fixture']
    if sql.startswith('SELECT name,if('):rows=[['items','table','Fixture comment','']]
    elif sql.startswith("SELECT name,'index'"):rows=[['items_idx','index','id','items']]
    elif sql.startswith('SELECT name,type,comment'):rows=[['id','UInt64','Key',1],['name','String','Label',0]]
    elif sql.startswith('SELECT name,expr,type'):rows=[['items_idx','id','minmax']]
    elif sql.startswith('SHOW CREATE TABLE'):rows=[['CREATE TABLE fixture.items (id UInt64, name String) ENGINE=MergeTree ORDER BY id']]
    elif sql.startswith('SELECT * FROM'):rows=[[i,'row '+str(i)] for i in range(101)]
    else:raise AssertionError(sql)
   elif sql.startswith('SELECT version()'):rows=[{'version':'24.8-http-contract'}]
   elif sql.startswith('SHOW DATABASES'):rows=[{'name':'fixture'}]
   elif sql.startswith('SELECT name,engine'):assert params['param_db']==['fixture'];rows=[{'name':'items','engine':'MergeTree'}]
   elif sql.startswith('SELECT name,type,default'):assert params['param_table']==['items'];rows=[{'name':'id','type':'UInt64','default_expression':''}]
   elif sql.startswith('SELECT name,expr,type'):rows=[]
   elif sql.startswith('CREATE DATABASE {new_database:Identifier}'):assert params['param_new_database'][0].startswith('probe_');rows=[]
   else:raise AssertionError(sql)
   if params.get('default_format') in [['JSON'],['JSONCompact']]:
    names=['c'+str(i) for i in range(len(rows[0]) if rows else 0)]
    body=json.dumps({'meta':[{'name':n,'type':'String'} for n in names],'data':[dict(zip(names,row)) for row in rows]}).encode()
   else:body=''.join(json.dumps(row)+'\n' for row in rows).encode()
   self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
 try:run_probe('clickhouse',server.server_port,'fixture');assert len(seen)>=5;print('PASS: ClickHouse HTTP contract (not a real ClickHouse server)')
 finally:server.shutdown();server.server_close()
