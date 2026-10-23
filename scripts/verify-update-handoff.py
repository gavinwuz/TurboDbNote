"""Exercise the standalone Windows updater using only owned mock programs and data.
No real installer or database is run. Covers both package flavors, process/mutex
waiting, cancellation, package locks, replacement, restart and rejected requests.
"""
import argparse,ctypes as c,hashlib,json,os,queue,shutil,subprocess,tempfile,threading,time,tomllib,sys
from pathlib import Path

PARENT=r'''use std::{io::{BufRead,Write},process::{Command,Stdio},time::Duration};
#[link(name="kernel32")] unsafe extern "system" {fn CreateMutexW(a:*const (),b:i32,c:*const u16)->*mut ();fn CloseHandle(h:*mut ())->i32;}
fn main(){
 let exe=std::env::current_exe().unwrap();
 if std::env::args().any(|a|a=="--update-completed"){std::fs::write(exe.with_file_name("restarted.marker"),b"reopened").unwrap();return;}
 let name:Vec<u16>="TurboDbNote.Running\0".encode_utf16().collect();
 let handle=unsafe{CreateMutexW(std::ptr::null(),0,name.as_ptr())};
 if std::env::args().any(|a|a=="--hold"){println!("holding");std::io::stdout().flush().unwrap();let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();unsafe{CloseHandle(handle);}return;}
 let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();
 let mut request:serde_json::Value=serde_json::from_str(&line).unwrap();
 request["parent"]=serde_json::json!(std::process::id());request["application"]=serde_json::json!(exe);
 if request["wrong_parent"]==true {request["parent"]=serde_json::json!(1);}
 request.as_object_mut().unwrap().remove("wrong_parent");
 let mut helper=Command::new(std::env::args().nth(1).unwrap()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
 helper.stdin.take().unwrap().write_all(&serde_json::to_vec(&request).unwrap()).unwrap();
 let stdout=helper.stdout.take().unwrap();let (tx,rx)=std::sync::mpsc::channel();
 std::thread::spawn(move||{let mut reader=std::io::BufReader::new(stdout);loop{let mut line=String::new();if reader.read_line(&mut line).unwrap_or(0)==0{break;}let value:serde_json::Value=serde_json::from_str(&line).unwrap();if value=="Ready"||value.get("Failed").is_some(){let _=tx.send(value);break;}}});
 let reply=rx.recv_timeout(Duration::from_secs(30)).unwrap();println!("{}",serde_json::json!({"ready":reply=="Ready","pid":helper.id(),"reply":reply}));std::io::stdout().flush().unwrap();
 line.clear();std::io::stdin().read_line(&mut line).unwrap();
 if line.trim()=="cancel"||reply!="Ready"{let _=helper.kill();let _=helper.wait();}
 unsafe{CloseHandle(handle);}
}
'''
INSTALLER=r'''fn main(){
 let exe=std::env::current_exe().unwrap();
 let target=std::env::args().find_map(|a|a.strip_prefix("/DIR=").map(str::to_owned)).unwrap();
 let app=std::path::Path::new(&target).join("turbodbnote.exe");
 let old=app.with_extension("old");std::fs::rename(&app,&old).unwrap();std::fs::rename(&old,&app).unwrap();
 std::fs::write(exe.with_file_name("installed.marker"),b"replacement succeeded").unwrap();
 std::thread::sleep(std::time::Duration::from_millis(250));
}
'''

def line(process,timeout=35):
    q=queue.Queue();threading.Thread(target=lambda:q.put(process.stdout.readline()),daemon=True).start()
    return q.get(timeout=timeout)

def wait_marker(path,timeout=15):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        if path.is_file():return
        time.sleep(.05)
    raise AssertionError('Mock expected marker was not produced: '+path.name)

def cancel_window(pid):
    user=c.WinDLL('user32');callback=c.WINFUNCTYPE(c.c_bool,c.c_void_p,c.c_ssize_t);found=[]
    @callback
    def visit(hwnd,_):
        owner=c.c_ulong();user.GetWindowThreadProcessId(c.c_void_p(hwnd),c.byref(owner))
        if owner.value==pid and user.IsWindowVisible(c.c_void_p(hwnd)):found.append(hwnd)
        return True
    user.EnumWindows(visit,0)
    assert found,'Helper status window was missing'
    for hwnd in found:user.PostMessageW(c.c_void_p(hwnd),0x10,0,0)

def capture_status(pid, case, work):
    from ctypes import wintypes as w
    user=c.WinDLL('user32');gdi=c.WinDLL('gdi32')
    callback=c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM);found=[]
    @callback
    def visit(hwnd,_):
        owner=w.DWORD();user.GetWindowThreadProcessId(hwnd,c.byref(owner))
        if owner.value==pid and user.IsWindowVisible(hwnd):found.append(hwnd)
        return True
    user.EnumWindows.argtypes=[callback,w.LPARAM];user.EnumWindows(visit,0)
    assert found,'Updater status window is missing'
    hwnd=found[0];labels=[]
    @callback
    def child(control,_):
        text=c.create_unicode_buffer(1024);user.GetWindowTextW(control,text,1024)
        if text.value:labels.append(text.value)
        return True
    user.EnumChildWindows.argtypes=[w.HWND,callback,w.LPARAM];user.EnumChildWindows(hwnd,child,0)
    assert any('关闭其他' in text for text in labels),labels
    (work/(case+'-status.json')).write_text(json.dumps(labels,ensure_ascii=False),encoding='utf-8')
    try:from PIL import Image
    except ImportError:return
    user.GetWindowRect.argtypes=[w.HWND,c.POINTER(w.RECT)]
    rect=w.RECT();user.GetWindowRect(hwnd,c.byref(rect));width=rect.right-rect.left;height=rect.bottom-rect.top
    user.GetWindowDC.argtypes=[w.HWND];user.GetWindowDC.restype=w.HDC
    user.ReleaseDC.argtypes=[w.HWND,w.HDC]
    gdi.CreateCompatibleDC.argtypes=[w.HDC];gdi.CreateCompatibleDC.restype=w.HDC
    gdi.CreateCompatibleBitmap.argtypes=[w.HDC,c.c_int,c.c_int];gdi.CreateCompatibleBitmap.restype=w.HBITMAP
    gdi.SelectObject.argtypes=[w.HDC,w.HGDIOBJ];gdi.SelectObject.restype=w.HGDIOBJ
    gdi.DeleteObject.argtypes=[w.HGDIOBJ];gdi.DeleteDC.argtypes=[w.HDC]
    user.PrintWindow.argtypes=[w.HWND,w.HDC,w.UINT]
    dc=user.GetWindowDC(hwnd);mem=gdi.CreateCompatibleDC(dc);bitmap=gdi.CreateCompatibleBitmap(dc,width,height);old=gdi.SelectObject(mem,bitmap)
    try:
        assert user.PrintWindow(hwnd,mem,2),'Cannot capture updater status'
        header=(c.c_ubyte*40)();import struct
        struct.pack_into('<IiiHHIIiiII',header,0,40,width,-height,1,32,0,width*height*4,0,0,0,0)
        pixels=c.create_string_buffer(width*height*4)
        gdi.GetDIBits.argtypes=[w.HDC,w.HBITMAP,w.UINT,w.UINT,c.c_void_p,c.c_void_p,w.UINT]
        assert gdi.GetDIBits(mem,bitmap,0,height,pixels,header,0)
        Image.frombuffer('RGB',(width,height),pixels,'raw','BGRX',0,1).save(work/(case+'-status.png'))
    finally:
        gdi.SelectObject(mem,old);gdi.DeleteObject(bitmap);gdi.DeleteDC(mem);user.ReleaseDC(hwnd,dc)

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--executable',type=Path,default=Path('target/debug/turbodbnote-update-helper.exe'));args=parser.parse_args()
    if os.name!='nt':raise SystemExit('Windows required')
    root=Path(__file__).resolve().parents[1];work=root/'target/gui-verification/updates';work.mkdir(parents=True,exist_ok=True)
    kernel=c.WinDLL('kernel32');kernel.OpenMutexW.restype=c.c_void_p;kernel.OpenMutexW.argtypes=[c.c_ulong,c.c_int,c.c_wchar_p];kernel.CloseHandle.argtypes=[c.c_void_p]
    kernel.OpenProcess.argtypes=[c.c_ulong,c.c_int,c.c_ulong];kernel.OpenProcess.restype=c.c_void_p
    kernel.WaitForSingleObject.argtypes=[c.c_void_p,c.c_ulong];kernel.TerminateProcess.argtypes=[c.c_void_p,c.c_uint]
    existing=kernel.OpenMutexW(0x00100000,False,'TurboDbNote.Running')
    if existing:kernel.CloseHandle(existing)
    cases=['full','minimal','no-restart','other-instance','concurrent','cancel','hash','identity','outside','parent']
    if existing:
        print('Existing user instance retained: success/restart cases deferred; verifying blocked handoff instead.')
        cases=['blocked','minimal-blocked','concurrent','cancel','hash','identity','outside','parent']
    # Compile the mock parent using the workspace's already-resolved serde_json library.
    project=work/'parent';(project/'src').mkdir(parents=True,exist_ok=True)
    lock=tomllib.loads((root/'Cargo.lock').read_text(encoding='utf-8'))
    serde_version=next(p['version'] for p in lock['package'] if p['name']=='serde_json')
    (project/'Cargo.toml').write_text('[package]\nname="update-handoff-fixture"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nserde_json="='+serde_version+'"\n')
    (project/'src/main.rs').write_text(PARENT,encoding='utf-8')
    subprocess.run(['cargo','generate-lockfile','--offline','--manifest-path',str(project/'Cargo.toml')],check=True,stdout=subprocess.DEVNULL)
    subprocess.run(['cargo','build','--locked','--offline','-j','1','--manifest-path',str(project/'Cargo.toml'),'--target-dir',str(work/'parent-target')],check=True,stdout=subprocess.DEVNULL)
    parent_exe=work/'parent-target/debug/update-handoff-fixture.exe'
    source=work/'installer-fixture.rs';source.write_text(INSTALLER,encoding='utf-8');installer=work/'installer-fixture.exe'
    subprocess.run(['rustc','--edition=2024',str(source),'-o',str(installer)],check=True)
    cache=Path(os.environ['LOCALAPPDATA'])/'TurboDbNote/updates';cache.mkdir(parents=True,exist_ok=True)
    for case in cases:
        with tempfile.TemporaryDirectory(prefix='standalone-handoff-',dir=cache) as tmp:
            directory=Path(tmp).resolve();directory.relative_to(cache.resolve())
            appdir=directory/'app';appdir.mkdir();app=appdir/'turbodbnote.exe';shutil.copy2(parent_exe,app);(appdir/'unins000.exe').write_bytes(b'mock uninstaller marker')
            helperdir=directory/'helper';helperdir.mkdir();helper=helperdir/'turbodbnote-update-helper.exe';shutil.copy2(args.executable.resolve(),helper)
            name='TurboDBNote-v0.0.1-windows-x64-'+('minimal-' if case.startswith('minimal') else '')+'setup.exe'
            if case=='identity':name='untrusted-setup.exe'
            package=directory/name
            if case=='outside':package=work/name
            shutil.copy2(installer,package)
            request={'path':str(package),'sha256':hashlib.sha256(package.read_bytes()).hexdigest(),'restart':case!='no-restart','language':'zh-CN'}
            if case=='hash':request['sha256']='0'*64
            if case=='parent':request['wrong_parent']=True
            parent=subprocess.Popen([str(app),str(helper)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
            holder=None;helper_handle=None
            try:
                parent.stdin.write(json.dumps(request)+'\n');parent.stdin.flush();reply=json.loads(line(parent))
                helper_handle=kernel.OpenProcess(0x00100001,False,reply['pid'])
                rejected=case in ['hash','identity','outside','parent']
                assert reply['ready']!=rejected,(case,reply)
                marker=package.with_name('installed.marker')
                assert not marker.exists(),'Installation started before parent exit'
                if rejected:
                    parent.stdin.write('cancel\n');parent.stdin.flush();parent.wait(timeout=10)
                else:
                    try:
                        with package.open('wb'):raise AssertionError('Verified installer was not locked')
                    except PermissionError:pass
                    if case in ['blocked','minimal-blocked']:
                        parent.stdin.write('exit\n');parent.stdin.flush();parent.wait(timeout=10)
                        time.sleep(.6);assert not marker.exists(),'User instance was ignored'
                        capture_status(reply['pid'],case,work)
                        cancel_window(reply['pid']);time.sleep(.5)
                    elif case=='concurrent':
                        second=subprocess.Popen([str(app),str(helper)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
                        try:
                            second.stdin.write(json.dumps(request)+'\n');second.stdin.flush();other=json.loads(line(second))
                            assert not other['ready'] and other['reply']=={'Failed':'updates.error.in_progress'},other
                            second.stdin.write('cancel\n');second.stdin.flush();second.wait(timeout=10)
                        finally:
                            if second.poll() is None:second.kill();second.wait(timeout=10)
                            second.stdin.close();second.stdout.close()
                        cancel_window(reply['pid']);time.sleep(.5)
                        parent.stdin.write('cancel\n');parent.stdin.flush();parent.wait(timeout=10)
                        assert not marker.exists()
                    elif case=='cancel':
                        cancel_window(reply['pid']);time.sleep(.5)
                        assert parent.poll() is None,'Cancellation ended the application'
                        parent.stdin.write('cancel\n');parent.stdin.flush();parent.wait(timeout=10)
                        assert not marker.exists()
                    else:
                        if case=='other-instance':
                            holder=subprocess.Popen([str(app),'--hold'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
                            assert line(holder).strip()=='holding'
                        parent.stdin.write('exit\n');parent.stdin.flush();parent.wait(timeout=10)
                        if holder:
                            time.sleep(.6);assert not marker.exists(),'Other instance was ignored'
                            capture_status(reply['pid'],case,work)
                            holder.stdin.write('exit\n');holder.stdin.flush();holder.wait(timeout=10)
                        wait_marker(marker)
                        if case!='no-restart':wait_marker(appdir/'restarted.marker')
                        else:time.sleep(.5);assert not (appdir/'restarted.marker').exists()
                print('PASS: standalone updater '+case)
            finally:
                if helper_handle and sys.exc_info()[0] is not None:
                    kernel.TerminateProcess(helper_handle,1)
                for process in [holder,parent]:
                    if process is not None:
                        if process.poll() is None:process.kill();process.wait(timeout=10)
                        if process.stdin:process.stdin.close()
                        if process.stdout:process.stdout.close()
                if helper_handle:
                    if kernel.WaitForSingleObject(helper_handle,10000)!=0:
                        kernel.TerminateProcess(helper_handle,1);kernel.WaitForSingleObject(helper_handle,5000)
                    kernel.CloseHandle(helper_handle)
                if case=='outside':package.unlink(missing_ok=True)

if __name__=='__main__':main()
