import pathlib,json,hashlib
out=pathlib.Path('/private/tmp/rumi-sec19-release-20261113');raw=out/'rumi_protocol_backend.wasm';opt=out/'rumi_protocol_backend.wasm'
data=opt.read_bytes(); pos=8; code=None
def leb(pos):
 n=0; shift=0
 while True:
  b=data[pos];pos+=1;n|=(b&127)<<shift
  if b<128:return n,pos
  shift+=7
while pos<len(data):
 sid=data[pos]; length,start=leb(pos+1)
 if sid==10:code=length
 pos=start+length
assert code is not None and code < 10*1024*1024-256*1024
report={'source':'4d8322a71059f146f6ec44c95add49789be2b6f8','deployed_base':'00cf550d3fcb97870cd4643883d5abd8900fa1a2','code_section_bytes':code,'project_guard_bytes':10*1024*1024-256*1024,'files':[{'name':p.name,'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in [opt,out/'rumi_protocol_backend.wasm.gz']]}
(out/'artifact-report.json').write_text(json.dumps(report,indent=2)); print(json.dumps(report))
