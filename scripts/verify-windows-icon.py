"""Inspect resource ID 1 in a built Windows EXE without launching it."""
import ctypes as c
from ctypes import wintypes as w
from pathlib import Path
import struct
import sys

exe = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/turbodbnote.exe").resolve()
kernel = c.WinDLL("kernel32", use_last_error=True)
kernel.LoadLibraryExW.argtypes = [w.LPCWSTR, w.HANDLE, w.DWORD]
kernel.LoadLibraryExW.restype = w.HMODULE
kernel.FindResourceW.argtypes = [w.HMODULE, c.c_void_p, c.c_void_p]
kernel.FindResourceW.restype = w.HRSRC
kernel.LoadResource.argtypes = [w.HMODULE, w.HRSRC]
kernel.LoadResource.restype = w.HGLOBAL
kernel.LockResource.argtypes = [w.HGLOBAL]
kernel.LockResource.restype = c.c_void_p
kernel.SizeofResource.argtypes = [w.HMODULE, w.HRSRC]
kernel.SizeofResource.restype = w.DWORD
kernel.FreeLibrary.argtypes = [w.HMODULE]
user = c.WinDLL("user32", use_last_error=True)
user.CreateIconFromResourceEx.argtypes = [c.c_void_p, w.DWORD, w.BOOL, w.DWORD, c.c_int, c.c_int, w.UINT]
user.CreateIconFromResourceEx.restype = w.HICON
user.DestroyIcon.argtypes = [w.HICON]
module = kernel.LoadLibraryExW(str(exe), None, 2)  # LOAD_LIBRARY_AS_DATAFILE
if not module:
    raise c.WinError(c.get_last_error())
try:
    group = kernel.FindResourceW(module, 1, 14)  # RT_GROUP_ICON
    assert group, "Missing icon group ID 1 (required by GPUI)"
    data = c.string_at(kernel.LockResource(kernel.LoadResource(module, group)),
                       kernel.SizeofResource(module, group))
    reserved, kind, count = struct.unpack_from("<HHH", data)
    assert (reserved, kind, count) == (0, 1, 8)
    sizes = []
    for index in range(count):
        width, height, _, _, _, bits, length, icon_id = struct.unpack_from("<BBBBHHIH", data, 6 + index * 14)
        resource = kernel.FindResourceW(module, icon_id, 3)  # RT_ICON
        assert resource and kernel.SizeofResource(module, resource) == length
        assert bits == 32 and width == height
        payload = c.string_at(kernel.LockResource(kernel.LoadResource(module, resource)), length)
        size = width or 256
        if size < 256:
            assert struct.unpack_from("<I", payload)[0] == 40, "Small icons must use DIB encoding"
        buffer = c.create_string_buffer(payload)
        native_icon = user.CreateIconFromResourceEx(buffer, length, True, 0x30000, size, size, 0)
        assert native_icon, f"Windows failed to decode {size}px icon"
        user.DestroyIcon(native_icon)
        sizes.append(width or 256)
    assert sorted(sizes) == [16, 20, 24, 32, 48, 64, 128, 256]
    print(f"PASS: {exe.name}: icon group 1, 32-bit sizes {sorted(sizes)}")
finally:
    kernel.FreeLibrary(module)
