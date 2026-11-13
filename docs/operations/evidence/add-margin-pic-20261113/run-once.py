import pathlib,subprocess,hashlib,json,datetime,os,time,signal,sys,shutil
src=pathlib.Path('/private/tmp/rumi-swap-support-cleanup-20261111');expected='7dd8cb29b878cec9d01f256457a631ff2b27f08a';out=pathlib.Path('/private/tmp/rumi-sec19-pic-20261113')
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
grant=pathlib.Path(sys.argv[1]);assert hashlib.sha256(grant.read_bytes()).hexdigest()==sys.argv[2];allocation=json.loads(grant.read_text());assert allocation['source']==expected;deadline=datetime.datetime.fromisoformat(allocation['startBeforeUtc'].replace('Z','+00:00'));assert datetime.datetime.now(datetime.timezone.utc)<deadline;assert hashlib.sha256((out/'steps.py').read_bytes()).hexdigest()=='34f845a7dfd0d150b11581974fb1c213dd7cc5fc35de2b96ef8f7c3a6fb3b524';assert hashlib.sha256((out/'tool-pins.json').read_bytes()).hexdigest()=='779c774111c53233705c334563894249cc2559bde4ffda28ba27076b498b6899';same_source();census();free0=shutil.disk_usage(src).free;assert free0>=20*1024**3
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
   elif time.monotonic()-start>840:reason='execution deadline before60s closure'
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
    if not reason:
     # The existing PocketIC harness leaves its localhost server after Cargo exits.
     # Only the exact pinned executable in this owned group is expected cleanup.
     allowed='/Users/robertripley/coding/rumi-protocol-v2/pocket-ic'
     commands=[]
     for row in children:
      q=subprocess.run(['ps','-p',str(row[1]),'-o','comm='],text=True,capture_output=True,timeout=10)
      if q.returncode==0:commands.append(q.stdout.strip())
     expected_pic=(code==0 and bool(commands) and all(str(pathlib.Path(c).resolve())==allowed for c in commands))
     record['expectedPocketIcCleanup']=expected_pic
     if not expected_pic and own(proc.pid):reason='unexpected surviving owned children after native exit' 
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
