#!/usr/bin/env python3
"""One frozen frontend build, inert without Cody's matching one-use allocation."""
import argparse, hashlib, json, os, pathlib, re, selectors, shutil, signal, subprocess, time
base=pathlib.Path(__file__).resolve().parent
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
request_path=base/'request.json'; request=json.loads(request_path.read_text())
root=pathlib.Path(request['root']); warm=pathlib.Path(request['warmDependencyRoot'])
p=argparse.ArgumentParser();p.add_argument('--allocation',required=True);p.add_argument('--allocation-sha256',required=True);a=p.parse_args()
ap=pathlib.Path(a.allocation);assert sha(ap)==a.allocation_sha256
allocation=json.loads(ap.read_text()); aid=allocation['allocationId'];assert re.fullmatch(r'[A-Za-z0-9-]+',aid)
assert allocation['ownerChat']==request['ownerChat']
assert allocation['source']==request['source'] and allocation['tree']==request['tree']
assert allocation['runnerSha256']==sha(pathlib.Path(__file__)) and allocation['requestSha256']==sha(request_path)
assert allocation['status']=='ISSUED' and allocation['unconsumedDelivery'] is True
assert time.time()<allocation['startBeforeEpoch']
live=json.loads(pathlib.Path('/private/tmp/elias-resource-allocation.json').read_text());grant=live['allowedLocalAllocation']
assert (grant.get('allocationId') if isinstance(grant,dict) else grant)==aid
assert live['localOwner']==allocation['owner']
def git(*args):return subprocess.check_output(['git',*args],cwd=request['gitObjectRoot'],text=True).strip()
def pins():
    assert git('rev-parse',request['source'])==request['source'] and git('rev-parse',request['source']+'^{tree}')==request['tree']
    for tree,key in [('src/vault_frontend','frontendTree'),('scripts','scriptsTree'),('src/declarations','declarationsTree')]:
        assert git('rev-parse',request['source']+':'+tree)==request[key]
    for name,digest in request['sourceFilesSha256'].items():assert sha(root/name)==digest,name
    assert sha(base/'source-export.json')==request['sourceExportSha256']
    export=json.loads((base/'source-export.json').read_text())
    assert export['sourceCommit']==request['source'] and export['exportPaths']==request['exportPaths']
    for f in export['exportedFiles']:assert sha(root/f['path'])==f['sha256'],f['path']
    assert git('diff','--name-only',request['previousBuildSource'],request['source'],'--','src/vault_frontend','scripts','src/declarations').splitlines()==request['allowedSourceDelta']
    assert (root/'node_modules').resolve(strict=True)==warm
    assert sha(warm/'.package-lock.json')==request['warmHiddenLockSha256']
    for name,v in request['packages'].items():assert sha(warm/name/'package.json')==v['packageSha256'],name
    assert subprocess.check_output(['node','--version'],text=True).strip()==request['node']
    assert subprocess.check_output(['npm','--version'],text=True).strip()==request['npm']
    assert shutil.which('npm')==request['npmExecutable']
    return True
def free_bytes():
    s=os.statvfs(root);return s.f_bavail*s.f_frsize
births={}
def members(pgid):
    raw=subprocess.check_output(['ps','-axo','pid=,ppid=,pgid=,rss=,stat=,lstart='],text=True);rows=[]
    for line in raw.splitlines():
        v=line.split()
        if len(v)!=10:raise RuntimeError('Malformed process census')
        rows.append((int(v[0]),int(v[1]),int(v[2]),int(v[3])*1024,v[4],tuple(v[5:])))
    owned={pid for pid,pp,g,r,s,b in rows if g==pgid or (pid in births and births[pid]==b)}
    while True:
        expanded=owned|{pid for pid,pp,g,r,s,b in rows if pp in owned}
        if expanded==owned:break
        owned=expanded
    result=[]
    for pid,pp,g,r,s,b in rows:
        if pid in owned and not s.startswith('Z'):births[pid]=b;result.append((pid,r))
    return result
def close_group(proc):
    try:os.killpg(proc.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    for sig in [signal.SIGTERM,signal.SIGKILL]:
        remaining=members(proc.pid)
        if not remaining:break
        for pid,_ in remaining:
            try:os.kill(pid,sig)
            except ProcessLookupError:pass
        end=time.monotonic()+25
        while members(proc.pid) and time.monotonic()<end:time.sleep(.25)
    proc.wait(timeout=5)
    return not members(proc.pid)
assert pins(); bounds=request['bounds'];start_free=free_bytes();assert start_free>=bounds['minimumFreeBytes']
out=base/('build-'+aid);out.mkdir(exist_ok=False)
with (base/('consumed-'+aid+'.json')).open('x') as f:json.dump({'allocationId':aid,'source':request['source'],'startedEpoch':time.time()},f)
start=time.monotonic();deadline=start+bounds['executionSeconds'];active=None
receipt={'allocationId':aid,'source':request['source'],'tree':request['tree'],'canonicalMerge':request['canonicalMerge'],'runnerSha256':sha(pathlib.Path(__file__)),'requestSha256':sha(request_path),'command':request['command'],'environment':request['environment'],'startedUtc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'startFreeBytes':start_free,'peakRssBytes':0,'maximumPhysicalGrowthBytes':0,'ownedGroupsClosed':False,'result':'STOPPED','productionMutation':False}
try:
    env=dict(os.environ,**request['environment']);env.pop('RUMI_REGEN_DID',None)
    with (out/'build.log').open('wb') as log:
        active=subprocess.Popen(request['command'],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,start_new_session=True)
        receipt['pid']=active.pid;selector=selectors.DefaultSelector();selector.register(active.stdout,selectors.EVENT_READ);written=0
        while selector.get_map() or active.poll() is None:
            free=free_bytes();rss=sum(r for _,r in members(active.pid));growth=max(0,start_free-free)
            receipt['peakRssBytes']=max(receipt['peakRssBytes'],rss);receipt['maximumPhysicalGrowthBytes']=max(receipt['maximumPhysicalGrowthBytes'],growth)
            if time.monotonic()>=deadline:raise RuntimeError('Execution deadline')
            if free<bounds['minimumFreeBytes'] or growth>bounds['physicalGrowthBytes'] or rss>bounds['ownedRssBytes']:raise RuntimeError('Resource bound')
            for key,_ in selector.select(.25):
                data=os.read(key.fileobj.fileno(),65536)
                if not data:selector.unregister(key.fileobj);continue
                if written+len(data)>bounds['logBytes']:raise RuntimeError('Log bound')
                log.write(data);written+=len(data)
        active.wait();receipt['exitCode']=active.returncode;receipt['ownedGroupsClosed']=close_group(active);active=None;selector.close()
    assert receipt['exitCode']==0 and receipt['ownedGroupsClosed'],'First causal build failure or unclosed descendants; no retry'
    logtext=(out/'build.log').read_text(errors='replace')
    assert 'Verified Internet Identity and custom-domain assets' in logtext
    assert 'Generated two discovery assets for 7 approved Rumi services (ic)' in logtext
    assert pins()
    dist=root/'src/vault_frontend/dist';assert dist.is_dir()
    ds=dist/'.DS_Store'
    if ds.exists():assert ds.is_file() and not ds.is_symlink();ds.unlink()
    shutil.copyfile(root/'src/vault_frontend/assets/favicon.ico',dist/'favicon.ico')
    assert sha(dist/'favicon.ico')==request['sourceFilesSha256']['src/vault_frontend/assets/favicon.ico']
    for name,digest in {'ic-architecture':'22bcdf6a5f1ad8e4c1b241a46e3a7f1ae5bec0ab16a79e62ce215288e862f702','ii-derivation-origin':'16cf6b5a8eb787b3222ba58308cd6234755b20be869337e1e11e42b51eee53a0'}.items():assert sha(dist/'.well-known'/name)==digest
    files=[]
    for path in sorted(dist.rglob('*')):
        if path.is_symlink():raise RuntimeError('Artifact symlink')
        if path.is_file():files.append({'path':path.relative_to(dist).as_posix(),'bytes':path.stat().st_size,'sha256':sha(path)})
    manifest={'source':request['source'],'canonicalMerge':request['canonicalMerge'],'frontendTree':request['frontendTree'],'scriptsTree':request['scriptsTree'],'files':files}
    (out/'artifact-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    receipt['artifactFiles']=len(files);receipt['artifactBytes']=sum(f['bytes'] for f in files);receipt['manifestSha256']=sha(out/'artifact-manifest.json')
    assert time.monotonic()<deadline
    receipt['result']='PASS'
except BaseException as e:
    receipt['error']=str(e)
finally:
    if active is not None:
        try:receipt['ownedGroupsClosed']=close_group(active)
        except BaseException as e:receipt['closureError']=str(e);receipt['ownedGroupsClosed']=False
    try:receipt['sourceAndDependenciesUnchanged']=pins()
    except BaseException as e:receipt['sourceAndDependenciesUnchanged']=False;receipt['pinError']=str(e)
    receipt['elapsedSeconds']=round(time.monotonic()-start,3);receipt['endFreeBytes']=free_bytes();receipt['physicalGrowthBytes']=max(0,start_free-receipt['endFreeBytes'])
    if not receipt['ownedGroupsClosed'] or not receipt['sourceAndDependenciesUnchanged'] or receipt['endFreeBytes']<bounds['minimumFreeBytes'] or receipt['physicalGrowthBytes']>bounds['physicalGrowthBytes'] or receipt['elapsedSeconds']>bounds['executionSeconds']+bounds['closureSeconds']:receipt['result']='STOPPED'
    (out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))
    raise SystemExit(0 if receipt['result']=='PASS' else 1)
