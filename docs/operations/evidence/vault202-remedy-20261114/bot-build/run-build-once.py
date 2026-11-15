import os,sys,json,time,signal,subprocess,hashlib,pathlib,datetime
W=pathlib.Path('/private/tmp/rumi-vault202-recovery-20261114');R=pathlib.Path('/private/tmp/vault202-validation-20261114');TARGET='/private/tmp/rumi-cksol-icp-target';HEAD='c4617b771dc40ccaf573ce47bbd48419233ec863';CARGO='/Users/robertripley/.cargo/bin/cargo';DIDC='/Users/robertripley/.cargo/bin/didc'
grant=json.loads(pathlib.Path(sys.argv[1]).read_text());assert grant['target']==TARGET and grant['source']==HEAD and grant['runnerSha256']==hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest();assert grant['jobs']==1 and grant['nativeAllowed'] is True;assert time.time()<=grant['startByEpoch'];assert grant['timeoutSeconds']<=1800 and grant['maxRssBytes']<=4294967296 and grant['maxFsDropBytes']<=2147483648 and grant['minFreeBytes']>=21474836480;assert TARGET and pathlib.Path(TARGET).is_dir();assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=W,text=True).strip()==HEAD;assert not subprocess.check_output(['git','status','--porcelain'],cwd=W)
for row in json.loads((R/'build-source-pins.json').read_text())['files']:assert hashlib.sha256((W/row['path']).read_bytes()).hexdigest()==row['sha256'],row['path']
O=R/('build-'+grant['allocationId']);O.mkdir(mode=0o700,exist_ok=False)
def free():
 s=os.statvfs(W);return s.f_bavail*s.f_frsize
def group_rows(pgid):
 rows=[]
 for line in subprocess.check_output(['ps','-axo','pid=,pgid=,rss='],text=True).splitlines():
  p,g,r=map(int,line.split())
  if g==pgid:rows.append((p,r*1024))
 return rows
base=free();assert base>=grant['minFreeBytes'];start=time.monotonic();env=os.environ.copy();env.update(CARGO_BUILD_JOBS='1',CARGO_TARGET_DIR=TARGET,CARGO_NET_OFFLINE='true');command=[CARGO,'build','--offline','--locked','--release','--target','wasm32-unknown-unknown','-p','liquidation_bot','--lib'];report={'allocationId':grant['allocationId'],'head':HEAD,'target':TARGET,'command':command,'startedUtc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'baselineFreeBytes':base,'peakOwnedRssBytes':0,'minimumFreeBytes':base,'maxFsDropBytes':0,'ownedPids':[],'nativeExit':None,'checks':[],'outcome':'running'}
def save():(O/'receipt.json').write_text(json.dumps(report,indent=2)+'\n')
seen=set();limit=None
with (O/'native.log').open('xb') as log:
 p=subprocess.Popen(command,cwd=W,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True);report['ownedPgid']=p.pid;save()
 while p.poll() is None:
  rows=group_rows(p.pid);seen.update(x[0] for x in rows);rss=sum(x[1] for x in rows);f=free();report['peakOwnedRssBytes']=max(report['peakOwnedRssBytes'],rss);report['minimumFreeBytes']=min(report['minimumFreeBytes'],f);report['maxFsDropBytes']=max(report['maxFsDropBytes'],base-f);report['ownedPids']=sorted(seen);save()
  if time.monotonic()-start>grant['timeoutSeconds']:limit='deadline'
  elif rss>grant['maxRssBytes']:limit='owned-rss'
  elif f<grant['minFreeBytes']:limit='disk-floor'
  elif base-f>grant['maxFsDropBytes']:limit='filesystem-drop'
  if limit:
   os.killpg(p.pid,signal.SIGTERM);break
  time.sleep(1)
 try:rc=p.wait(timeout=20)
 except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);rc=p.wait()
 report['nativeExit']=rc;report['resourceStop']=limit
 close=time.monotonic()
 while group_rows(p.pid) and time.monotonic()-close<30:time.sleep(0.2)
 remaining=group_rows(p.pid);report['ownedGroupClosed']=not remaining;report['remainingOwnedPids']=[x[0] for x in remaining];report['elapsedSeconds']=time.monotonic()-start;save()
if rc or limit or remaining:
 report['outcome']='stopped-or-native-failed';save();print(json.dumps(report));sys.exit(rc if rc and rc>0 else 1)
ICW='/Users/robertripley/.cargo/bin/ic-wasm'
raw=pathlib.Path(TARGET)/'wasm32-unknown-unknown/release/liquidation_bot.wasm';assert raw.is_file()
import shutil
shutil.copy2(raw,O/'raw.wasm')
steps=[['shrink',[ICW,str(O/'raw.wasm'),'-o',str(O/'shrunk.wasm'),'shrink']],['candid-metadata',[ICW,str(O/'shrunk.wasm'),'-o',str(O/'candidate.wasm'),'metadata','candid:service','-f',str(W/'src/liquidation_bot/liquidation_bot.did'),'-v','public']],['artifact-info',[ICW,str(O/'candidate.wasm'),'info']],['endpoint-check',[ICW,str(O/'candidate.wasm'),'check-endpoints','--candid',str(W/'src/liquidation_bot/liquidation_bot.did'),'--hidden','<ic-cdk internal> timer_executor']]]
for name,cmd in steps:
 with (O/(name+'.log')).open('xb') as log:r=subprocess.run(cmd,cwd=W,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=120)
 report['checks'].append({'name':name,'exit':r.returncode});save()
 if r.returncode:report['outcome']='mechanical-check-failed';save();print(json.dumps(report));sys.exit(r.returncode)
report['artifacts']=[{'name':n,'bytes':(O/n).stat().st_size,'sha256':hashlib.sha256((O/n).read_bytes()).hexdigest()}for n in ['raw.wasm','shrunk.wasm','candidate.wasm']]
report['outcome']='build-and-artifact-pass';report['finishedUtc']=datetime.datetime.now(datetime.timezone.utc).isoformat();save();print(json.dumps(report))
