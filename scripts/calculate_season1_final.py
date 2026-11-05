"""Offline proposed make-whole calculation. Never sends update calls."""
import json,re,pathlib,hashlib
import argparse
_parser=argparse.ArgumentParser();_parser.add_argument('--evidence-dir',required=True);P=pathlib.Path(_parser.parse_args().evidence_dir)
START=1792972800000000000; END=1793491199000000000; DAY=86400000000000
rows=json.loads((P/'comparison-not-allocation.json').read_text())['rows']
def integer(s):return int(s.replace('_',''))
def number(text,field):
 m=re.search(r'\b'+field+r' = ([\d_]+) : nat',text)
 if not m:raise ValueError('missing '+field)
 return integer(m.group(1))
def ledger_balance(text,ledger):
 matches=re.findall(r'principal "'+ledger+r'";\s*([\d_]+) : nat64;',text)
 if len(matches)>1:raise ValueError('duplicate balance')
 return integer(matches[0]) if matches else 0
changes={}
for f in (P/'backend-pages').glob('*.candid'):
 for b in re.split(r'\brecord \{\s*(?=\d[\d_]* : nat64;\s*variant \{)',f.read_text())[1:]:
  m=re.search(r'timestamp = opt \(([\d_]+) : nat64\)',b)
  if not m:raise ValueError('event without timestamp')
  t=integer(m.group(1));kind=re.search(r'variant \{\s*(\w+)\s*=',b).group(1)
  if t<START:continue
  if kind not in ('open_vault','borrow_from_vault','repay_to_vault'):raise ValueError('unhandled debt event '+kind)
  if kind=='open_vault':continue
  owner=re.search(r'caller = opt principal "([a-z0-9-]+)"',b).group(1)
  amount=number(b,'borrowed_amount' if kind=='borrow_from_vault' else 'repayed_amount')
  changes.setdefault(owner,[]).append((t,amount if kind=='borrow_from_vault' else -amount))
for r in rows:
 owner=r['principal'];root=P/'current-positions';point=(root/(owner+'-points.candid')).read_text();sp=(root/(owner+'-sp.candid')).read_text();vaults=(root/(owner+'-vaults.candid')).read_text();wallet=(root/(owner+'-wallet.candid')).read_text()
 assert 'repayment_events = vec {}' in point
 debt=sum(integer(x) for x in re.findall(r'borrowed_icusd_amount = ([\d_]+) : nat64',vaults))
 events=sorted(changes.get(owner,[]));balance=debt-sum(amount for _,amount in events);assert balance>=0
 # Current observed interest is deliberately allowed over the whole interval;
 # only recorded principal changes are moved at their actual timestamps.
 cursor=START;debt_ns=0
 for t,amount in events:
  capped=min(t,END);debt_ns+=balance*(capped-cursor);balance+=amount;assert balance>=0;cursor=capped
 debt_ns+=balance*(END-cursor)
 sp_i=ledger_balance(sp,'t6bor-paaaa-aaaap-qrd5q-cai');sp_t=ledger_balance(sp,'fohh4-yyaaa-aaaap-qtkpa-cai')
 raw_wallet=integer(re.search(r'\(([\d_]+) : nat\)',wallet).group(1));verified_positive=raw_wallet+sp_t>0
 components={}
 for chunk in point.split('record {')[1:]:
  if 'recorded_value_usd' in chunk:
   asset=re.search(r'asset = variant \{ (\w+) \}',chunk).group(1)
   if 'venue = variant { ThreePool }' not in chunk:raise ValueError('unexpected venue')
   components[asset]=number(chunk,'recorded_value_usd')
 pool=(components.get('IcUsd',0)+4*(components.get('CkUsdc',0)+components.get('CkUsdt',0))) if verified_positive else 0
 duration=END-START;parts={'debt':debt_ns//DAY,'recorded_pool_cap':pool*duration//DAY,'icusd_sp_current_ceiling':sp_i*duration//DAY,'threeusd_sp':2*sp_t*duration//DAY}
 r.update({'components_e8':parts,'additional_points_e8':sum(parts.values()),'before_points_e8':number(point,'total_points')})
 r['proposed_final_points_e8']=r['before_points_e8']+r['additional_points_e8']
report={'status':'PROPOSED compensation; not exact counterfactual snapshots or an approved allocation','method':'Recorded debt changes timed exactly; current debt interest allowed across whole interval; full recorded 3pool composition cap if continuous positive 3USD holdings; current Stability Pool balance ceiling; unchanged 3USD balance and no AMM liquidity events. Historical source/valuation assumptions remain explicit.','start_ns':START,'end_ns':END,'before_total_e8':sum(r['before_points_e8'] for r in rows),'additional_total_e8':sum(r['additional_points_e8'] for r in rows),'rows':rows}
assert report['before_total_e8']==429409727203403
(P/'proposed-final-allocation.json').write_text(json.dumps(report,indent=2)+'\n')
print('additional',report['additional_total_e8']/1e8,'final',(report['before_total_e8']+report['additional_total_e8'])/1e8)
print('sha256',hashlib.sha256((P/'proposed-final-allocation.json').read_bytes()).hexdigest())
