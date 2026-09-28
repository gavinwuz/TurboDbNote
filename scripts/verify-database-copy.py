"""Isolated legacy MySQL copy verification. Explicit local tool paths; no user databases."""
import argparse, pathlib, tempfile, shutil, socket, subprocess, time, re
parser=argparse.ArgumentParser();parser.add_argument('--server-dir',type=pathlib.Path,required=True);parser.add_argument('--client-dir',type=pathlib.Path,required=True);parser.add_argument('--probe',type=pathlib.Path,required=True);args=parser.parse_args()
root=pathlib.Path(__file__).resolve().parents[1]
fixture=pathlib.Path(tempfile.mkdtemp(prefix='copy-mysql-',dir=root/'target')).resolve()
assert fixture.is_relative_to((root/'target').resolve())
server_root=args.server_dir.resolve();client_root=args.client_dir.resolve();shutil.copytree(server_root/'data',fixture/'data')
server=server_root/'bin/mysqld.exe'
if not server.exists():server=server_root/'bin/mysqld-nt.exe'
with socket.socket() as listener:listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
log=(fixture/'server.log').open('wb')
process=subprocess.Popen([str(server),'--no-defaults','--basedir='+str(server_root),'--datadir='+str(fixture/'data'),'--bind-address=127.0.0.1','--port='+str(port),'--console','--innodb-data-home-dir='+str(fixture/'data'),'--innodb-log-group-home-dir='+str(fixture/'data')],cwd=fixture,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
def sql(statement):
 command=[str(client_root/'bin/mysql.exe'),'--no-defaults','--protocol=TCP','--host=127.0.0.1','--port='+str(port),'--user=root','--batch','--skip-column-names']
 command.append('--password='+str())
 result=subprocess.run(command,input=statement.encode(),capture_output=True,timeout=30,creationflags=subprocess.CREATE_NO_WINDOW)
 if result.returncode:raise RuntimeError('fixture SQL command failed: '+re.sub(r"(['\"]).*?\1",'<redacted>',result.stderr.decode(errors='replace'))[:200])
 return result.stdout.decode().strip()
try:
 deadline=time.monotonic()+30
 while time.monotonic()<deadline:
  if process.poll() is not None:raise RuntimeError('fixture server exited')
  try:
   with socket.create_connection(('127.0.0.1',port),timeout=.3):break
  except OSError:time.sleep(.2)
 else:raise RuntimeError('fixture startup timeout')
 actual=pathlib.Path(sql("SHOW VARIABLES LIKE 'datadir'").split("\t",1)[1]).resolve()
 if actual!=(fixture/'data').resolve():raise RuntimeError('fixture datadir mismatch')
 sql("CREATE DATABASE source_db; USE source_db; CREATE TABLE `parent`(`id` INT NOT NULL,`label` VARCHAR(20),PRIMARY KEY(`id`)) TYPE=InnoDB; CREATE TABLE `child`(`id` INT NOT NULL,`parent_id` INT,PRIMARY KEY(`id`),KEY `parent_lookup` (`parent_id`),CONSTRAINT `fk_parent` FOREIGN KEY(`parent_id`) REFERENCES `parent`(`id`)) TYPE=InnoDB; INSERT INTO `parent` VALUES(1,'fixture'); INSERT INTO `child` VALUES(10,1);")
 version=sql('SELECT VERSION()')
 if not version.startswith('4.0'):sql('CREATE VIEW source_db.example_view AS SELECT * FROM source_db.parent')
 result=subprocess.run([str(args.probe.resolve()),str(port),str(client_root/'bin/libmysql.dll')],cwd=root,capture_output=True,timeout=120,creationflags=subprocess.CREATE_NO_WINDOW)
 if result.returncode:raise RuntimeError('copy probe failed: '+result.stderr.decode(errors='replace')[-1600:])
 assert sql('SELECT count(*) FROM source_db.parent')=='1'
 assert sql('SELECT count(*) FROM full_copy.child')=='1'
 if not version.startswith('4.0'):assert sql("SELECT DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME='charset_copy'")=='gbk'
 print('PASS: isolated MySQL copy; fixture',fixture)
finally:
 if process.poll() is None:process.terminate();process.wait(timeout=15)
 log.close()
