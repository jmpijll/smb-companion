"""Windows integration tests against a built companion, using disposable Modrinth data only."""
import ctypes
from contextlib import closing
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import unittest
import zipfile

BINARY = Path(os.environ.get('SMB_TEST_BINARY', 'target/debug/smb-lol.exe')).resolve()
USER32 = ctypes.WinDLL('user32', use_last_error=True)
USER32.GetForegroundWindow.restype = wintypes.HWND
USER32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
CALLBACK = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
USER32.EnumWindows.argtypes = [CALLBACK, wintypes.LPARAM]

class HeadlessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='smb-headless-')
        self.root = Path(self.temp.name)
        self.base = self.root / 'ModrinthApp'
        self.profile = self.base / 'profiles' / 'smb'
        self.profile.mkdir(parents=True)
        self.settings = self.root / 'SMB' / 'LoL Companion' / 'minecraft.json'
        self.settings.parent.mkdir(parents=True)
        self.config = dict(instance_id='local:fixture', automatic=True, direct_hook=True, launch_after_update=True)
        with closing(sqlite3.connect(self.base / 'app.db')) as c:
            c.executescript("""
CREATE TABLE instances(id TEXT,path TEXT,applied_content_set_id TEXT,name TEXT);
CREATE TABLE instance_content_sets(id TEXT,game_version TEXT,loader TEXT,loader_version TEXT);
CREATE TABLE instance_links(instance_id TEXT,link_kind TEXT,imported_name TEXT,imported_version_number TEXT);
CREATE TABLE instance_launch_overrides(instance_id TEXT,overrides TEXT);
CREATE TABLE settings(hook_pre_launch TEXT,custom_dir TEXT);
CREATE TABLE instance_sync_preferences(instance_id TEXT,feature TEXT,enabled INTEGER);
CREATE TABLE sync_feature_settings(feature TEXT,globally_enabled INTEGER,new_instance_default INTEGER);
INSERT INTO instances VALUES('local:fixture','smb','set','SMB');
INSERT INTO instance_content_sets VALUES('set','26.3','fabric','0.20.1');
INSERT INTO instance_links VALUES('local:fixture','imported_modpack','SMB','0.5.0');
INSERT INTO settings VALUES(NULL,NULL);
""")
            c.commit()
        self.channel = self.root / 'feed' / 'stable.json'
        self.channel.parent.mkdir()
        old = self.archive('0.5.0', {'mods/old.jar': b'old mod', 'config/test.json': b'old defaults'})
        new = self.archive('0.13.0', {'mods/new.jar': b'new mod', 'config/test.json': b'new defaults'})
        self.channel.write_text(json.dumps(dict(schema=1,pack=new,history=[old])))
        self.write('mods/old.jar', b'old mod')
        self.write('config/test.json', b'player settings')
        self.write('mods/personal.jar', b'personal mod')
        self.write('saves/world/level.dat', b'world stays')
        self.env = dict(os.environ,APPDATA=str(self.root),INST_DIR=str(self.profile),SMB_MINECRAFT_CHANNEL=str(self.channel))

    def tearDown(self): self.temp.cleanup()
    def write(self,path,data):
        p=self.profile/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(data)
    def archive(self,version,files,runtime='26.3'):
        p=self.channel.parent/f'smb-{version}.mrpack'
        with zipfile.ZipFile(p,'w',zipfile.ZIP_DEFLATED) as z:
            z.writestr('modrinth.index.json',json.dumps(dict(formatVersion=1,game='minecraft',name='SMB',versionId=version,dependencies={'minecraft':runtime,'fabric-loader':'0.20.1'},files=[])))
            for path,data in files.items():z.writestr('overrides/'+path,data)
        b=p.read_bytes()
        return dict(version=version,min_version='0.11.0' if version=='0.13.0' else version,url=p.name,sha256=hashlib.sha256(b).hexdigest(),size=len(b),minecraft=runtime,loader='fabric',loader_version='0.20.1')
    def run_hook(self,expected):
        self.settings.write_text(json.dumps(self.config))
        before=USER32.GetForegroundWindow()
        process=subprocess.Popen([str(BINARY),'--minecraft-prelaunch'],env=self.env,creationflags=subprocess.CREATE_NO_WINDOW)
        windows=[]
        @CALLBACK
        def inspect(hwnd,_):
            pid=wintypes.DWORD();USER32.GetWindowThreadProcessId(hwnd,ctypes.byref(pid))
            if pid.value==process.pid:windows.append(int(hwnd))
            return True
        deadline=time.monotonic()+15
        while process.poll() is None:
            USER32.EnumWindows(inspect,0)
            if time.monotonic()>deadline:process.kill();self.fail('Headless helper exceeded fixture deadline')
            time.sleep(.01)
        log=self.settings.parent/'activity.log'
        self.assertEqual(process.returncode,expected,log.read_text() if log.exists() else '')
        self.assertEqual(windows,[],'Headless helper created a native window')
        self.assertEqual(USER32.GetForegroundWindow(),before,'Headless helper changed focus')
    def require_unblocked(self):
        log=self.settings.parent/'activity.log'
        if log.exists() and 'another Java app is running' in log.read_text():
            self.skipTest('Java is active; real guard correctly blocked before this later check')
    def test_full_update_is_synchronous_and_preserves_personal_files(self):
        try:self.run_hook(0)
        except AssertionError:
            log=self.settings.parent/'activity.log'
            if log.exists() and 'another Java app is running' in log.read_text():self.skipTest('Java is active; guard correctly blocked. Full pipeline is covered by Rust disposable-profile integration.')
            raise
        self.assertFalse((self.profile/'mods/old.jar').exists())
        self.assertEqual((self.profile/'mods/new.jar').read_bytes(),b'new mod')
        self.assertEqual((self.profile/'config/test.json').read_bytes(),b'player settings')
        self.assertEqual((self.profile/'mods/personal.jar').read_bytes(),b'personal mod')
        self.assertEqual((self.profile/'saves/world/level.dat').read_bytes(),b'world stays')
        state=json.loads((self.profile/'.smb-companion/state.json').read_text())
        self.assertEqual(state['version'],'0.13.0')
        self.run_hook(0)
    def test_disabled_helper_makes_no_profile_writes_and_pending_blocks(self):
        self.config['direct_hook']=False
        self.run_hook(0)
        self.assertFalse((self.profile/'.smb-companion').exists())
        self.write('.smb-companion/journal.json',b'{}')
        self.run_hook(73)
        self.assertEqual((self.profile/'mods/old.jar').read_bytes(),b'old mod')
    def test_wrong_instance_and_unavailable_channel_block_without_gui(self):
        other=self.root/'other';other.mkdir();self.env['INST_DIR']=str(other)
        self.run_hook(73)
        self.assertFalse((self.profile/'.smb-companion').exists())
        self.env['INST_DIR']=str(self.profile);self.channel.unlink()
        self.run_hook(73)
        self.assertEqual((self.profile/'mods/old.jar').read_bytes(),b'old mod')
        self.assertFalse((self.profile/'mods/new.jar').exists())
        self.require_unblocked()
        self.assertIn('local channel', (self.settings.parent/'activity.log').read_text())
    def test_runtime_change_blocks_before_pack_mutation(self):
        metadata=json.loads(self.channel.read_text())
        metadata['pack']=self.archive('0.13.0',{'mods/new.jar':b'new mod'},runtime='26.4')
        self.channel.write_text(json.dumps(metadata))
        self.run_hook(73)
        self.assertEqual((self.profile/'mods/old.jar').read_bytes(),b'old mod')
        self.assertFalse((self.profile/'mods/new.jar').exists())
        self.require_unblocked()
        self.assertIn('26.4', (self.settings.parent/'activity.log').read_text())

if __name__=='__main__':unittest.main()
