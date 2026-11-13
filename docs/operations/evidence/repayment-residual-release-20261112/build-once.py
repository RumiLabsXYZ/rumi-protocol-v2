import pathlib,subprocess,hashlib,json,datetime,os,time,signal,sys,shutil
src=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.worktrees/xrp-customer-withdrawal-address-20261106');expected='00cf550d3fcb97870cd4643883d5abd8900fa1a2';out=pathlib.Path('/private/tmp/rumi-sec17-release-20261112')
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def census():
 rows=subprocess.check_output(['ps','-axo','pgid=,pid=,rss='],text=True,timeout=10).splitlines();parsed=[]
 for row in rows:
  cols=row.split();assert len(cols)==3 and all(x.isdigit() for x in cols),'invalid ps census';parsed.append(tuple(map(int,cols)))
 assert parsed,'empty ps census';return parsed
def own(pgid):return [x for x in census() if x[0]==pgid]
def same_source():
 assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=src,text=True).strip()==expected
 assert not subprocess.check_output(['git','diff','--','src','Cargo.toml','Cargo.lock','package.json','package-lock.json'],cwd=src)
assert hashlib.sha256((out/'build-steps.py').read_bytes()).hexdigest()=='314ba63bfc0c2e4f33a489719f47cf2c19f55ac3f541ff70dea26b64b6f5a1a7'
grant=pathlib.Path(sys.argv[1]);assert hashlib.sha256(grant.read_bytes()).hexdigest()==sys.argv[2];allocation=json.loads(grant.read_text());assert allocation['source']==expected;deadline=datetime.datetime.fromisoformat(allocation['startBeforeUtc'].replace('Z','+00:00'));assert datetime.datetime.now(datetime.timezone.utc)<deadline;same_source();census();free0=shutil.disk_usage(src).free;assert free0>=20*1024**3
with (out/'build-dispatched').open('x') as f:f.write(now());f.flush();os.fsync(f.fileno())
start=time.monotonic();record={'source':expected,'command':['python3','-I',str(out/'build-steps.py')],'start':now(),'allocationSha256':sys.argv[2],'allocationId':allocation['allocationId'],'initialFreeBytes':free0};peak=0;minfree=free0;reason=None;proc=None;code=None;closed=False;exception=None
try:
 with (out/'frontend-build.log').open('wb') as log:
  env=os.environ.copy();env['NODE_OPTIONS']='--max-old-space-size=6144';env['npm_config_offline']='true';env['npm_config_update_notifier']='false'
  proc=subprocess.Popen(record['command'],cwd=src,stdout=log,stderr=subprocess.STDOUT,start_new_session=True,env=env);record['pgid']=proc.pid
  while proc.poll() is None:
   rss=sum(x[2]*1024 for x in own(proc.pid));peak=max(peak,rss);free=shutil.disk_usage(src).free;minfree=min(minfree,free)
   if rss>8*1024**3:reason='owned RSS limit'
   elif free<20*1024**3:reason='disk floor'
   elif free0-free>12*1024**3:reason='physical growth bound'
   elif time.monotonic()-start>1740:reason='execution deadline before60s closure'
   if reason:break
   time.sleep(1)
  code=proc.poll()
except BaseException as e:exception=repr(e);reason='runner or monitor exception'
finally:
 if proc:
  # Cleanup must not depend on the monitor that may have failed.
  if exception or reason:
   try:os.killpg(proc.pid,signal.SIGTERM)
   except ProcessLookupError:pass
  try:
   children=own(proc.pid)
   if proc.poll() is None or children:
    if not reason:reason='surviving owned children after native exit'
    try:os.killpg(proc.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    until=time.monotonic()+25
    while time.monotonic()<until and own(proc.pid):time.sleep(.25)
    if own(proc.pid):
     try:os.killpg(proc.pid,signal.SIGKILL)
     except ProcessLookupError:pass
   code=proc.wait(timeout=15);closed=not own(proc.pid)
  except BaseException as e:
   exception=(exception or '')+' closure:'+repr(e);closed=False
   try:os.killpg(proc.pid,signal.SIGKILL)
   except ProcessLookupError:pass
   try:code=proc.wait(timeout=15)
   except BaseException:pass
 free=shutil.disk_usage(src).free;minfree=min(minfree,free);record.update(end=now(),exit=code,stopReason=reason,exception=exception,peakOwnedRssBytes=peak,minFreeBytes=minfree,physicalGrowthBytes=max(0,free0-minfree),allOwnedClosed=closed,seconds=time.monotonic()-start);(out/'build-session.json').write_text(json.dumps(record,indent=2)+'\n')
assert code==0 and not reason and not exception and closed,'build failed or held; no automatic retry';same_source()
dist=src/'src/vault_frontend/dist';files=[{'path':str(p.relative_to(dist)),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(dist.rglob('*')) if p.is_file()];(out/'frontend-manifest.json').write_text(json.dumps({'source':expected,'files':files},indent=2)+'\n');print('One sequential production backend/frontend/auth batch passed;',len(files),'manifest files; owned group closed')
