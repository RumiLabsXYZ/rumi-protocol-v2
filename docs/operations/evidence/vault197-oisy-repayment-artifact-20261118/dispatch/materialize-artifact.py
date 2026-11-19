#!/usr/bin/env python3
"""Local file extraction only; no network, build, install or asset write."""
import argparse,hashlib,json,tarfile,pathlib
p=argparse.ArgumentParser();p.add_argument('archive');p.add_argument('manifest');p.add_argument('output');a=p.parse_args()
archive=pathlib.Path(a.archive);manifest=pathlib.Path(a.manifest)
assert hashlib.sha256(archive.read_bytes()).hexdigest()=='3915e35932d1b47df5a8a8345c61c1416fc4a26fec4ea3642a8a54913a4ac82e'
assert hashlib.sha256(manifest.read_bytes()).hexdigest()=='c0e20f5654625c32ca442e1be7dc49119d5cf9de0622be85740440e0a0bae6ae'
expected={f['path']:f for f in json.loads(manifest.read_text())['files']};out=pathlib.Path(a.output);out.mkdir(exist_ok=False)
with tarfile.open(archive,'r:gz') as tar:
    members=tar.getmembers();assert len(members)==len(expected)==764
    assert set(m.name for m in members)==set(expected)
    for m in members:
        q=pathlib.PurePosixPath(m.name);assert m.isfile() and not q.is_absolute() and '..' not in q.parts
        b=tar.extractfile(m).read();want=expected[m.name]
        assert len(b)==want['bytes'] and hashlib.sha256(b).hexdigest()==want['sha256']
        dest=out/pathlib.Path(*q.parts);dest.parent.mkdir(parents=True,exist_ok=True)
        with dest.open('xb') as f:f.write(b)
        dest.chmod(0o444)
print('All 764 frozen files materialized and verified; no build or live mutation.')
