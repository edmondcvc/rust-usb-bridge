"""Inspect the PE import table; clean PCs must not need a VC redistributable."""
import json, pathlib, struct, sys

def imports(path):
    data = pathlib.Path(path).read_bytes()
    u16 = lambda offset: struct.unpack_from('<H', data, offset)[0]
    u32 = lambda offset: struct.unpack_from('<I', data, offset)[0]
    pe = u32(0x3c)
    assert data[pe:pe+4] == b'PE\0\0'
    optional = pe + 24
    assert u16(optional) == 0x20b, 'Expected x64 executable'
    sections = optional + u16(pe+20)
    def offset(rva):
        for i in range(u16(pe+6)):
            section = sections + 40*i
            start = u32(section+12)
            size = max(u32(section+8),u32(section+16))
            if start <= rva < start+size:
                return u32(section+20)+rva-start
        raise ValueError('Invalid import RVA')
    result=[]
    table_rva = u32(optional+120)
    if table_rva:
        table = offset(table_rva)
        while any(data[table:table+20]):
            name = offset(u32(table+12))
            result.append(data[name:data.index(0,name)].decode('ascii'))
            table += 20
    return sorted(result)

report = []
for path in sys.argv[1:]:
    modules = imports(path)
    bad = [name for name in modules if name.lower().startswith(('vcruntime','msvcp','concrt'))]
    report.append({'file':path,'imports':modules,'external_vc_runtime':bad})
print(json.dumps(report,indent=2))
assert report and all(not item['external_vc_runtime'] for item in report), 'External VC runtime dependency remains'
