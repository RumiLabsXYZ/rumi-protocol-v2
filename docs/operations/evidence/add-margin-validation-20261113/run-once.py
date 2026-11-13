import pathlib,subprocess,hashlib,json,datetime,os,time,signal,sys,shutil
src=pathlib.Path('/private/tmp/rumi-swap-support-cleanup-20261111');expected='5c23d82ab64b5d6c7d4595952ec5a43dd6cf7324';out=pathlib.Path('/private/tmp/rumi-sec19-validation-20261113')
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def census():
 rows=subprocess.check_output(['ps','-axo','pgid=,pid=,rss='],text=True,timeout=10).splitlines();parsed=[]
 for row in rows:
  cols=row.split();assert len(cols)==3 and all(x.isdigit() for x in cols),'invalid ps census';parsed.append(tuple(map(int,cols)))
 assert parsed,'empty ps census';return parsed
def own(pgid):return [x for x in census() if x[0]==pgid]
def same_source():
 assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=src,text=True).strip()==expected
 assert not subprocess.check_output(['git','diff',expected,'--','Cargo.toml','Cargo.lock','src/rumi_protocol_backend','src/rumi_cycle_manager','src/flaky_ledger','src/xrc_demo/xrc/xrc.wasm'],cwd=src), 'backend source changed'
grant=pathlib.Path(sys.argv[1]);assert hashlib.sha256(grant.read_bytes()).hexdigest()==sys.argv[2];allocation=json.loads(grant.read_text());assert allocation['source']==expected;deadline=datetime.datetime.fromisoformat(allocation['startBeforeUtc'].replace('Z','+00:00'));assert datetime.datetime.now(datetime.timezone.utc)<deadline;assert hashlib.sha256((out/'steps.py').read_bytes()).hexdigest()=='b9d99572c11e9c1b51d1f750fbe7421fc2f3e9828dce5dd71b9062e8857e0d6a';assert hashlib.sha256((out/'tool-pins.json').read_bytes()).hexdigest()=='779c774111c53233705c334563894249cc2559bde4ffda28ba27076b498b6899';same_source();census();free0=shutil.disk_usage(src).free;assert free0>=20*1024**3
with (out/'build-dispatched').open('x') as f:f.write(now());f.flush();os.fsync(f.fileno())
start=time.monotonic();record={'source':expected,'command':['python3','-I',str(out/'steps.py')],'start':now(),'allocationSha256':sys.argv[2],'allocationId':allocation['allocationId'],'initialFreeBytes':free0};peak=0;minfree=free0;reason=None;proc=None;code=None;closed=False;exception=None
try:
 with (out/'validation.log').open('wb') as log:
  env=os.environ.copy();env['NODE_OPTIONS']='--max-old-space-size=6144';env['npm_config_offline']='true';env['npm_config_update_notifier']='false'
  proc=subprocess.Popen(record['command'],cwd=src,stdout=log,stderr=subprocess.STDOUT,start_new_session=True,env=env);record['pgid']=proc.pid
  while proc.poll() is None:
   rss=sum(x[2]*1024 for x in own(proc.pid));peak=max(peak,rss);free=shutil.disk_usage(src).free;minfree=min(minfree,free)
   if rss>8*1024**3:reason='owned RSS limit'
   elif free<20*1024**3:reason='disk floor'
   elif free0-free>8*1024**3:reason='physical growth bound'
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
 free=shutil.disk_usage(src).free;minfree=min(minfree,free);record.update(end=now(),exit=code,stopReason=reason,exception=exception,peakOwnedRssBytes=peak,minFreeBytes=minfree,physicalGrowthBytes=max(0,free0-minfree),allOwnedClosed=closed,seconds=time.monotonic()-start);(out/'validation-session.json').write_text(json.dumps(record,indent=2)+'\n')
assert code==0 and not reason and not exception and closed,'validation failed or held; no automatic retry'
print('Bounded SEC19 validation completed; owned group closed')
