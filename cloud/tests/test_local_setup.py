import json
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]

class LocalSetupTests(unittest.TestCase):
    def test_repeated_initialization_preserves_secrets_and_settings(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp)
            (root/'scripts').mkdir();(root/'docker').mkdir()
            shutil.copyfile(ROOT/'scripts/init-local.py',root/'scripts/init-local.py')
            shutil.copyfile(ROOT/'docker/realm-lightcraft.json',root/'docker/realm-lightcraft.json')
            def run():return subprocess.run([sys.executable,str(root/'scripts/init-local.py')],capture_output=True,text=True,check=True)
            first=run()
            files={p.name:p.read_text() for p in (root/'.secrets').iterdir() if p.name.endswith('password')}
            self.assertEqual(len(files),5)
            (root/'.env').write_text('LC_STORAGE_QUOTA_BYTES=123456789\n')
            second=run()
            self.assertEqual({name:(root/'.secrets'/name).read_text() for name in files},files)
            self.assertEqual((root/'.env').read_text(),'LC_STORAGE_QUOTA_BYTES=123456789\n')
            self.assertEqual(stat.S_IMODE((root/'.secrets').stat().st_mode),0o700)
            for name,password in files.items():
                self.assertEqual(stat.S_IMODE((root/'.secrets'/name).stat().st_mode),0o444)
                self.assertNotIn(password.strip(),first.stdout+first.stderr+second.stdout+second.stderr)
            realm=json.loads((root/'.secrets/lightcraft-dev-realm.json').read_text())
            self.assertEqual(realm['sslRequired'],'none')
            self.assertFalse(realm['clients'][0]['directAccessGrantsEnabled'])
