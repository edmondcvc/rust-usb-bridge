"""Release-path regressions: architecture, Unix permissions and volume integrity."""
import importlib.util,json,pathlib,tarfile,tempfile,unittest
ROOT=pathlib.Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('release_tools',ROOT/'scripts/release.py')
release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)
class ReleaseTests(unittest.TestCase):
    def test_package_rejects_cross_architecture_binary(self):
        with tempfile.TemporaryDirectory() as folder:
            p=pathlib.Path(folder); elf=bytearray(64);elf[:6]=b'\x7fELF\x02\x01';elf[18:20]=(62).to_bytes(2,'little');binary=p/'server';binary.write_bytes(elf)
            with self.assertRaisesRegex(AssertionError,'architecture mismatch'):release.package(binary,'aarch64',p)
            self.assertFalse((p/'stage-linux-aarch64').exists())
    def test_windows_packaging_preserves_executable_linux_tar_mode(self):
        with tempfile.TemporaryDirectory() as folder:
            p=pathlib.Path(folder);elf=bytearray(64);elf[:6]=b'\x7fELF\x02\x01';elf[18:20]=(183).to_bytes(2,'little');binary=p/'server';binary.write_bytes(elf)
            release.package(binary,'aarch64',p)
            with tarfile.open(p/f'RustUSBBridge-Linux-{release.VERSION}-aarch64.tar.gz') as tar:
                self.assertEqual(tar.getmember('usb-bridge-server').mode,0o755)
                self.assertEqual(tar.getmember('scripts/linux/install-linux.sh').mode,0o755)
                manifest=json.load(tar.extractfile('release.json'))
                self.assertEqual(manifest['arch'],'aarch64')
                sums=tar.extractfile('SHA256SUMS').read().decode()
                self.assertIn(release.digest(binary)+'  usb-bridge-server',sums)
    def test_bad_volume_never_creates_output(self):
        with tempfile.TemporaryDirectory() as folder:
            p=pathlib.Path(folder);(p/'image.part001').write_bytes(b'damaged')
            m={'file':'image.img.gz','size':7,'sha256':'0'*64,'parts':[{'name':'image.part001','size':7,'sha256':'0'*64}]}
            (p/'volumes.json').write_text(json.dumps(m))
            with self.assertRaisesRegex(AssertionError,'Bad part'):release.merge(p/'volumes.json',p)
            self.assertFalse((p/'image.img.gz').exists())
    def test_volume_path_cannot_escape_directory(self):
        with tempfile.TemporaryDirectory() as folder:
            p=pathlib.Path(folder);(p/'volumes.json').write_text(json.dumps({'file':'image.img.gz','parts':[{'name':'../outside'}]}))
            with self.assertRaisesRegex(AssertionError,'Unsafe part'):release.merge(p/'volumes.json',p)
if __name__=='__main__':unittest.main()
