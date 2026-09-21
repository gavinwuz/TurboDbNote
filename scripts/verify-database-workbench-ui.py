"""Windows native UI smoke test at 100% display scale, with an isolated writable SQLite fixture.
Build `cargo build -p turbodbnote --bins --locked -j 1` first. Requires Pillow.
It controls only the window belonging to its copied executable. Telemetry and AI auto-connect are disabled.
"""
import ctypes as c
from ctypes import wintypes as w
from pathlib import Path
import subprocess,shutil,time,json,argparse
from PIL import Image
import uuid
parser=argparse.ArgumentParser();parser.add_argument('--executable',type=Path,default=Path(__file__).resolve().parents[1]/'target/debug/turbodbnote.exe');args=parser.parse_args()
root=(Path(__file__).resolve().parents[1]/'target'/('workbench-preview-'+uuid.uuid4().hex[:8])).resolve();root.mkdir()
shutil.copy2(args.executable,root/'turbodbnote.exe')
(root/'portable.flag').write_text('isolated review')
(root/'crashsight.local.json').write_text('{"enabled":false}',encoding='utf-8')
(root/'data').mkdir()
(root/'data/settings.json').write_text(json.dumps({'dark':True,'general':{'language':'zh-CN'},'ai':{'codex':{'auto_connect':False},'claude':{'auto_connect':False}}}),encoding='utf-8')
user=c.WinDLL('user32');gdi=c.WinDLL('gdi32')
user.EnumWindows.argtypes=[c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM),w.LPARAM]
user.GetWindowThreadProcessId.argtypes=[w.HWND,c.POINTER(w.DWORD)]
user.PostMessageW.argtypes=[w.HWND,w.UINT,w.WPARAM,w.LPARAM]
user.GetWindowRect.argtypes=[w.HWND,c.POINTER(w.RECT)]
user.PrintWindow.argtypes=[w.HWND,w.HDC,w.UINT]
user.GetWindowDC.argtypes=[w.HWND];user.GetWindowDC.restype=w.HDC
user.ReleaseDC.argtypes=[w.HWND,w.HDC]
gdi.CreateCompatibleDC.argtypes=[w.HDC];gdi.CreateCompatibleDC.restype=w.HDC
gdi.CreateCompatibleBitmap.argtypes=[w.HDC,c.c_int,c.c_int];gdi.CreateCompatibleBitmap.restype=w.HBITMAP
gdi.SelectObject.argtypes=[w.HDC,w.HGDIOBJ];gdi.SelectObject.restype=w.HGDIOBJ
gdi.GetBitmapBits.argtypes=[w.HBITMAP,w.LONG,c.c_void_p]
gdi.DeleteObject.argtypes=[w.HGDIOBJ];gdi.DeleteDC.argtypes=[w.HDC]
startup=subprocess.STARTUPINFO();startup.dwFlags=subprocess.STARTF_USESHOWWINDOW;startup.wShowWindow=4
log=(root/'stderr.log').open('w')
import sqlite3
fixture=root/'fixture.db'
with sqlite3.connect(fixture) as db:
 db.executescript("CREATE TABLE teams(id INTEGER PRIMARY KEY, name TEXT); CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT DEFAULT 'kept', team_id INTEGER REFERENCES teams(id)); CREATE INDEX item_name ON items(name); CREATE VIEW items_view AS SELECT id,name FROM items; CREATE TRIGGER items_log AFTER INSERT ON items BEGIN SELECT 1; END;")
 db.executemany('INSERT INTO items(id,name) VALUES (?,?)',[(i,None if i==1 else 'row '+str(i)) for i in range(1,206)])
ids=[str(uuid.uuid4()) for _ in range(3)]
profiles=[{'id':ids[0],'kind':'Sqlite','name':'Local notes','group':'Development','environment':'dev','sqlite_path':str(fixture),'sqlite_read_only':False,'port':1,'user':''},
 {'id':ids[1],'kind':'PostgreSql','name':'Orders','group':'Business','environment':'prod','host':'127.0.0.1','port':5432,'user':'postgres','database':'postgres'},
 {'id':ids[2],'kind':'Redis','name':'Cache','group':'Cache','environment':'test','host':'127.0.0.1','port':6379,'user':'','database':'0'}]
(root/'data/connections.json').write_text(json.dumps({'version':2,'connections':profiles}),encoding='utf-8')
(root/'data/database-navigation.json').write_text(json.dumps({'recent':[ids[0]],'created':{ids[0]:'2026-09-20',ids[1]:'2026-09-19'}}),encoding='utf-8')
legacy_bytes=(root/'data/connections.json').read_bytes()
app=subprocess.Popen([str(root/'turbodbnote.exe')],cwd=root,startupinfo=startup,stderr=log)
windows=[]
@c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM)
def visit(hwnd,_):
 pid=w.DWORD();user.GetWindowThreadProcessId(hwnd,c.byref(pid))
 if pid.value==app.pid:windows.append(hwnd)
 return True
for _ in range(50):
 user.EnumWindows(visit,0)
 if windows:break
 time.sleep(.2)
assert windows
hwnd=windows[0]
user.ClientToScreen.argtypes=[w.HWND,c.POINTER(w.POINT)]
origin=w.POINT(0,0);user.ClientToScreen(hwnd,c.byref(origin));frame=w.RECT();user.GetWindowRect(hwnd,c.byref(frame))
dx=origin.x-frame.left;dy=origin.y-frame.top
# Screenshots include invisible Windows frame borders; WM mouse messages use client coordinates.
def point_at(x,y):return ((y-dy)<<16)|(x-dx)
def click(x,y):
 point=point_at(x,y)
 user.PostMessageW(hwnd,0x200,0,point);user.PostMessageW(hwnd,0x201,1,point);user.PostMessageW(hwnd,0x202,0,point)
 time.sleep(.6);assert app.poll() is None,(root/'stderr.log').read_text()
def capture(name):
 rect=w.RECT();user.GetWindowRect(hwnd,c.byref(rect));width=rect.right-rect.left;height=rect.bottom-rect.top
 dc=user.GetWindowDC(hwnd);memory=gdi.CreateCompatibleDC(dc);bitmap=gdi.CreateCompatibleBitmap(dc,width,height);old=gdi.SelectObject(memory,bitmap)
 try:
  user.PrintWindow(hwnd,memory,2)
  data=c.create_string_buffer(width*height*4);gdi.GetBitmapBits(bitmap,len(data),data)
  screenshot=Image.frombuffer('RGB',(width,height),data.raw,'raw','BGRX',0,1);screenshot.save(root/name);return screenshot
 finally:gdi.SelectObject(memory,old);gdi.DeleteObject(bitmap);gdi.DeleteDC(memory);user.ReleaseDC(hwnd,dc)
time.sleep(2)
def close():
 if app.poll() is None:
  user.PostMessageW(hwnd,0x10,0,0)
  try:app.wait(timeout=5)
  except subprocess.TimeoutExpired:app.terminate();app.wait()
 log.close()

def double(x,y):
 point=point_at(x,y)
 for _ in range(2):
  user.PostMessageW(hwnd,0x200,0,point);user.PostMessageW(hwnd,0x201,1,point);user.PostMessageW(hwnd,0x202,0,point);time.sleep(.08)
 time.sleep(.8)
def drag(x,y,xx,yy):
 user.PostMessageW(hwnd,0x200,0,point_at(x,y));user.PostMessageW(hwnd,0x201,1,point_at(x,y))
 for i in range(1,11):
  nx=x+(xx-x)*i//10;ny=y+(yy-y)*i//10;user.PostMessageW(hwnd,0x200,1,point_at(nx,ny));time.sleep(.06)
 user.PostMessageW(hwnd,0x202,0,point_at(xx,yy));time.sleep(.6)
def primary_y(left,right,top,bottom):
 screenshot=capture('button-position.png')
 rows=[y for y in range(top,bottom) if sum(all(v>210 for v in screenshot.getpixel((x,y))) for x in range(left,right))>(right-left)*.8]
 assert rows,'Primary button not visible in isolated test window'
 return (min(rows)+max(rows))//2
def nav():return json.loads((root/'data/database-navigation.json').read_text(encoding='utf-8'))
try:
 click(24,104);capture('initial.png')
 with sqlite3.connect(root/'data/connections.db') as db:
  assert db.execute('select count(*) from connections').fetchone()[0]==3
  assert db.execute('pragma user_version').fetchone()[0]==3
 assert (root/'data/connections.json').read_bytes()==legacy_bytes
 double(160,202);time.sleep(1);capture('connected.png')
 double(160,585);capture('grid.png')
 click(507,818);capture('page-2.png')
 click(87,585);time.sleep(.5);capture('expanded.png')
 double(170,671);capture('column-editor.png')
 click(880,237)
 for ch in '_new':user.PostMessageW(hwnd,0x102,ord(ch),0)
 time.sleep(.2);click(900,495);capture('sql-preview.png')
 with sqlite3.connect(fixture) as db:assert db.execute('pragma table_info(items)').fetchall()[1][1]=='name','Preview wrote to the database'
 execute_y=primary_y(990,1065,500,780);click(1027,execute_y);capture('risk-confirmation.png')
 with sqlite3.connect(fixture) as db:assert db.execute('pragma table_info(items)').fetchall()[1][1]=='name','Confirmation opened but already wrote data'
 confirm_y=primary_y(885,954,200,500);click(815,confirm_y)
 with sqlite3.connect(fixture) as db:assert db.execute('pragma table_info(items)').fetchall()[1][1]=='name','Cancelling confirmation wrote data'
 click(1027,execute_y);primary_y(885,954,200,500)
 user.PostMessageW(hwnd,0x100,0x0D,0);user.PostMessageW(hwnd,0x101,0x0D,0);time.sleep(.7)
 with sqlite3.connect(fixture) as db:
  col=db.execute('pragma table_info(items)').fetchall()[1];assert col[1]=='name_new' and col[4]=="'kept'",col
 click(288,54);click(330,120);capture('environment.png');assert nav()['mode']=='Environment'
 click(288,54);click(330,148);capture('date.png');assert nav()['mode']=='Date'
 click(288,54);click(330,176);capture('used-date.png');assert nav()['mode']=='UsedDate'
 with sqlite3.connect(root/'data/connections.db') as db:assert db.execute('select last_used_at from connections where id=?',(ids[0],)).fetchone()[0] is not None
 click(288,54);click(330,92);assert nav()['mode']=='Group'
 drag(192,410,192,510)
 drag(281,430,200,258);capture('reordered.png');assert nav()['order'][0]=='Development',nav()
 drag(192,510,192,305);capture('resized.png');assert 150<nav()['top_height']<300 and 0<nav()['top_ratio']<1,nav()
 assert app.poll() is None
 saved=nav();close()
 log=(root/'stderr.log').open('a')
 app=subprocess.Popen([str(root/'turbodbnote.exe')],cwd=root,startupinfo=startup,stderr=log)
 windows.clear()
 for _ in range(50):
  user.EnumWindows(visit,0)
  if windows:break
  time.sleep(.2)
 assert windows
 hwnd=windows[0];time.sleep(2);click(24,104);capture('restored.png')
 assert nav()['order']==saved['order'] and nav()['top_ratio']==saved['top_ratio']
 print('PASS: SQLite migration, native sidebar, paged tables, explicit SQL confirmation/cancellation, usage dates, grouping, reorder and split persistence')
 print('Screenshots:',root)
finally:close()
