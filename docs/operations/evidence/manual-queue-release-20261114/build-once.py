import pathlib,subprocess,hashlib,json,datetime,os,time,signal,sys,shutil
src=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.worktrees/xrp-customer-withdrawal-address-20261106');expected='e7b736c92de4749a204da3bb0dd7da809fa7c6fe';out=pathlib.Path('/private/tmp/rumi-manual-queue-frontend-release-20261114')
registry=pathlib.Path('/private/tmp/elias-resource-allocation.json');owner='OriginalDorian_SEC21FrontendBuild';owner_chat='01a1a800-7fde-71b2-a128-2b2325c4b232';source_tree='416e4fcceca8c14e6f93d5422c694c83277778cb'
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def census(timeout=10):
 rows=subprocess.check_output(['ps','-axo','pgid=,pid=,rss='],text=True,timeout=timeout).splitlines();parsed=[]
 for row in rows:
  cols=row.split();assert len(cols)==3 and all(x.isdigit() for x in cols),'invalid ps census';parsed.append(tuple(map(int,cols)))
 assert parsed,'empty ps census';return parsed
def own(pgid,timeout=10):return [x for x in census(timeout) if x[0]==pgid]
def same_source():
 assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=src,text=True).strip()==expected
 assert not subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=src), 'tracked source changed'
def live_grant(grant,digest,allocation):
 assert hashlib.sha256(grant.read_bytes()).hexdigest()==digest,'grant bytes changed'
 r=json.loads(registry.read_text());a=r['allowedLocalAllocation']
 assert r['reservationOwner']==owner,'local reservation belongs to another owner'
 assert isinstance(a,dict) and a['status']=='ISSUED_UNCONSUMED','allocation is not issued/unconsumed'
 assert a['consumed'] is False and a['released'] is False,'allocation consumed or released'
 assert a['deliveryState']=='ACCEPTED_DELIVERED_ONCE','allocation has not been accepted and delivered'
 assert a['allocationId']==allocation['allocationId'] and a['sha256']==digest,'active allocation UUID/hash mismatch'
 assert pathlib.Path(a['packet']).resolve()==grant.resolve(),'active allocation path mismatch'
 for key,want in [('owner',owner),('ownerChat',owner_chat),('source',expected),('sourceTree',source_tree)]:
  assert a[key]==want and allocation[key]==want,'active allocation identity mismatch: '+key
 assert allocation['runnerSha256']==hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'runner mismatch'
 deadline=datetime.datetime.fromisoformat(allocation['startBeforeUtc'].replace('Z','+00:00'))
 assert datetime.datetime.now(datetime.timezone.utc)<deadline,'allocation start deadline passed'
 return {'registry':str(registry),'allocationId':a['allocationId'],'sha256':a['sha256'],'owner':a['owner'],'ownerChat':a['ownerChat'],'status':a['status'],'consumed':a['consumed'],'released':a['released'],'deliveryState':a['deliveryState'],'verifiedUtc':now()}
class BuildStopped(BaseException):pass
receivedSignal=None;spawning=False;closing=False;previous_handlers={}
record={'source':expected,'command':['npm','run','build','--workspace=src/vault_frontend'],'runnerPid':os.getpid(),'dispatchStarted':False};peak=0;minfree=None;free0=None;reason=None;proc=None;pgid=None;code=None;closed=False;exception=None;start=time.monotonic()
def on_signal(signum,_frame):
 global receivedSignal
 first=receivedSignal is None
 if first:receivedSignal=signum
 record['receivedSignal']=receivedSignal
 # Never interrupt construction before ownership is captured, or cleanup.
 if first and not spawning and not closing:raise BuildStopped('received signal '+str(signum))
def save_receipt():
 record.update(end=now(),exit=code,stopReason=reason,exception=exception,receivedSignal=receivedSignal,pgid=pgid,peakOwnedRssBytes=peak,minFreeBytes=minfree,physicalGrowthBytes=max(0,free0-minfree) if free0 is not None and minfree is not None else 0,allOwnedClosed=closed,seconds=time.monotonic()-start)
 with (out/'build-session.json').open('w') as f:json.dump(record,f,indent=2);f.write('\n');f.flush();os.fsync(f.fileno())
try:
 for sig in [signal.SIGTERM,signal.SIGINT]:previous_handlers[sig]=signal.signal(sig,on_signal)
 grant=pathlib.Path(sys.argv[1]);digest=sys.argv[2];allocation=json.loads(grant.read_text())
 same_source();census();free0=shutil.disk_usage(src).free;minfree=free0;assert free0>=20*1024**3
 # This is the last admission check before the exclusive dispatch marker.
 record['activeAllocationCheck']=live_grant(grant,digest,allocation)
 record.update(start=now(),allocationSha256=digest,allocationId=allocation['allocationId'],initialFreeBytes=free0)
 with (out/'build-dispatched').open('x') as f:f.write(now());f.flush();os.fsync(f.fileno())
 dfd=os.open(out,os.O_RDONLY);os.fsync(dfd);os.close(dfd);record['dispatchStarted']=True;start=time.monotonic()
 try:
  with (out/'frontend-build.log').open('wb') as log:
   env=os.environ.copy();env['NODE_OPTIONS']='--max-old-space-size=6144';env['npm_config_offline']='true';env['npm_config_update_notifier']='false'
   spawning=True
   try:
    proc=subprocess.Popen(record['command'],cwd=src,stdout=log,stderr=subprocess.STDOUT,start_new_session=True,env=env)
    pgid=proc.pid;record['pgid']=pgid
    with (out/'owned-process.json').open('x') as f:json.dump({'runnerPid':os.getpid(),'pgid':pgid,'allocationId':allocation['allocationId'],'utc':now()},f);f.flush();os.fsync(f.fileno())
   finally:spawning=False
   if receivedSignal is not None:raise BuildStopped('signal deferred until PID ownership recorded')
   while proc.poll() is None:
    rss=sum(x[2]*1024 for x in own(pgid));peak=max(peak,rss);free=shutil.disk_usage(src).free;minfree=min(minfree,free)
    if rss>8*1024**3:reason='owned RSS limit'
    elif free<20*1024**3:reason='disk floor'
    elif free0-free>8*1024**3:reason='physical growth bound'
    elif time.monotonic()-start>840:reason='execution deadline before60s closure'
    if reason:break
    time.sleep(1)
   code=proc.poll()
  closing=True
 except BaseException as e:
  closing=True;exception=repr(e);reason='runner or monitor exception'
 finally:
  closing=True
  if proc is not None and pgid is not None:
   # All signals now only record. Cleanup has its own short census timeouts.
   try:
    children=own(pgid,2)
    if exception or reason or proc.poll() is None or children:
     if not reason:reason='surviving owned children after native exit'
     try:os.killpg(pgid,signal.SIGTERM)
     except ProcessLookupError:pass
     until=time.monotonic()+20
     while time.monotonic()<until and own(pgid,2):time.sleep(.25)
     if own(pgid,2):
      try:os.killpg(pgid,signal.SIGKILL)
      except ProcessLookupError:pass
    code=proc.wait(timeout=10);closed=not own(pgid,2)
   except BaseException as e:
    exception=(exception or '')+' closure:'+repr(e);closed=False
    try:os.killpg(pgid,signal.SIGKILL)
    except ProcessLookupError:pass
    try:code=proc.wait(timeout=5);closed=not own(pgid,2)
    except BaseException:pass
  else:closed=True
  free=shutil.disk_usage(src).free;minfree=min(minfree,free) if minfree is not None else free;save_receipt()
 closing=False
 assert code==0 and not reason and not exception and closed and receivedSignal is None,'build failed or held; no automatic retry'
 same_source()
 dist=src/'src/vault_frontend/dist';files=[{'path':str(p.relative_to(dist)),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(dist.rglob('*')) if p.is_file()]
 (out/'frontend-manifest.json').write_text(json.dumps({'source':expected,'files':files},indent=2)+'\n')
except BaseException as e:
 closing=True
 if exception is None:exception=repr(e)
 if reason is None:reason='runner exception or admission refusal'
 raise
finally:
 closing=True
 save_receipt()
 for sig,old_handler in previous_handlers.items():signal.signal(sig,old_handler)
save_receipt()
assert receivedSignal is None and code==0 and not reason and not exception and closed,'caught signal or failed closure prevents PASS'
print('One production frontend/auth build passed;',len(files),'manifest files; owned group closed')
