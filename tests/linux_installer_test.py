"""Run the real installer in an isolated filesystem with fake kernel/systemd tools.

No test invokes host systemctl/modprobe or writes /opt, /etc, /var or /sys.
"""
import sys,os,pathlib,shutil,subprocess,tempfile,unittest
ROOT=pathlib.Path(__file__).resolve().parent.parent
@unittest.skipUnless(sys.platform=='linux' and shutil.which('cc') and shutil.which('bash'),'requires a Linux C compiler and Bash')
class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=pathlib.Path(self.temp.name)
        self.bin=self.root/'bin';self.bin.mkdir();self.scripts=self.root/'scripts';self.scripts.mkdir()
        replacements={
            '/opt/rust-usb-bridge':str(self.root/'install'),
            '/var/lib/rust-usb-bridge':str(self.root/'state'),
            '/var/log/rust-usb-bridge':str(self.root/'logs'),
            '/etc/systemd/system':str(self.root/'units'),
            '/sys/bus/usb':str(self.root/'sys/bus/usb'),
        }
        (self.root/'units').mkdir();(self.root/'sys/bus/usb/drivers/usbip-host').mkdir(parents=True)
        for original in (ROOT/'scripts/linux').glob('*.sh'):
            content=original.read_text(encoding='utf-8').replace('[[ $EUID -eq 0 ]]','[[ 0 -eq 0 ]]')
            for before,after in replacements.items():content=content.replace(before,after)
            (self.scripts/original.name).write_text(content)
        (self.scripts/'check-host.sh').write_text('#!/bin/bash\nexit "${FIXTURE_PREREQUISITE_EXIT:-0}"\n')
        self.stub('uname','echo "${FIXTURE_ARCH:-x86_64}"')
        self.stub('findmnt','echo /dev/fixture-root')
        self.stub('modprobe','exit 0')
        self.stub('systemctl',r'''echo "$*" >> "$FIXTURE_ROOT/systemctl.log"
case "$1" in
is-active) test -f "$FIXTURE_ROOT/active";;
is-enabled) test -f "$FIXTURE_ROOT/enabled";;
start) touch "$FIXTURE_ROOT/active";; stop) rm -f "$FIXTURE_ROOT/active";;
enable) touch "$FIXTURE_ROOT/enabled";; disable) rm -f "$FIXTURE_ROOT/enabled";;
*) exit 0;; esac''')
        code=r'''#include <stdio.h>
#include <string.h>
int main(int n,char**v){const char*path=0;for(int i=1;i+1<n;i++)if(!strcmp(v[i],"--state-dir"))path=v[i+1];
if(n>1&&!strcmp(v[1],"restore"))return 0;if(!path)return 2;
const char*files[]={"config.json","ca.pem","ca-key.pem","server.pem","server-key.pem","fingerprint.txt"};
for(int i=0;i<6;i++){char name[4096];snprintf(name,sizeof(name),"%s/%s",path,files[i]);FILE*f=fopen(name,"w");if(!f)return 3;fputs("fixture-identity",f);fclose(f);}return 0;}'''
        (self.root/'server.c').write_text(code);self.binary=self.root/'server'
        subprocess.run(['cc',str(self.root/'server.c'),'-o',str(self.binary)],check=True)
        self.env=os.environ|{'PATH':str(self.bin)+os.pathsep+os.environ['PATH'],'FIXTURE_ROOT':str(self.root)}
    def stub(self,name,body):
        p=self.bin/name;p.write_text('#!/bin/bash\n'+body+'\n');p.chmod(0o755)
    def install(self,*args,env=None):
        return subprocess.run(['bash',str(self.scripts/'install-linux.sh'),'--binary',str(self.binary),*args],env=env or self.env,capture_output=True,text=True)
    def test_fresh_install_no_start_and_existing_pairing_preserved(self):
        r=self.install('--no-start');self.assertEqual(r.returncode,0,r.stderr+r.stdout)
        identity=(self.root/'state/ca-key.pem').read_bytes()
        (self.root/'state/config.json').write_text('existing-pairings')
        r=self.install();self.assertEqual(r.returncode,0,r.stderr+r.stdout)
        self.assertEqual((self.root/'state/ca-key.pem').read_bytes(),identity)
        self.assertEqual((self.root/'state/config.json').read_text(),'existing-pairings')
        self.assertTrue((self.root/'active').exists())
    def test_wrong_elf_architecture_fails_before_install(self):
        r=self.install(env=self.env|{'FIXTURE_ARCH':'aarch64'});self.assertNotEqual(r.returncode,0)
        self.assertFalse((self.root/'install').exists())
    def test_missing_prerequisites_fail_before_install(self):
        r=self.install(env=self.env|{'FIXTURE_PREREQUISITE_EXIT':'1'});self.assertNotEqual(r.returncode,0)
        self.assertFalse((self.root/'install').exists())
    def test_bad_listener_is_not_written(self):
        r=self.install('--listen','bad address:3241');self.assertNotEqual(r.returncode,0)
        self.assertFalse((self.root/'units/rust-usb-bridge.service').exists())
    def test_busy_device_prevents_service_stop(self):
        r=self.install();self.assertEqual(r.returncode,0,r.stderr+r.stdout)
        status=self.root/'sys/bus/usb/devices/1-2/usbip_status';status.parent.mkdir(parents=True);status.write_text('2\n')
        log=self.root/'systemctl.log';log.write_text('')
        r=self.install();self.assertNotEqual(r.returncode,0)
        self.assertNotIn('stop rust-usb-bridge.service',log.read_text())
        self.assertTrue((self.root/'active').exists())
    def test_missing_backup_does_not_stop_service(self):
        r=self.install();self.assertEqual(r.returncode,0,r.stderr+r.stdout)
        log=self.root/'systemctl.log';log.write_text('')
        r=subprocess.run(['bash',str(self.scripts/'rollback-linux.sh'),'--backup','20000101T000000Z'],env=self.env,capture_output=True,text=True)
        self.assertNotEqual(r.returncode,0);self.assertNotIn('stop ',log.read_text())
        self.assertTrue((self.root/'active').exists())
if __name__=='__main__':unittest.main()
