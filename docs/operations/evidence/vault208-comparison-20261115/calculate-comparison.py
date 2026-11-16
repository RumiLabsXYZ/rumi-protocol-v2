from decimal import Decimal as D, getcontext, ROUND_CEILING
from pathlib import Path
import json
getcontext().prec=55
P=D('0.937824869'); R=D('1.32266666666666666624'); B=D('1.075'); F=D('0.03'); E8=D(10)**8; E6=D(10)**6
C=3499000010; debt=281245001091
continuous=(R*D(debt)/E8-D(C)/E6*P)/(R-B)
repay=int((continuous*E8).to_integral_value(rounding=ROUND_CEILING))
def sized(n):
 base=int(D(n)/E8/P*E6); gross=int(D(base)*B); fee=int(D(gross-base)*F); remaining_debt=debt-n; remaining=C-gross
 return {'debtRetiredE8s':n,'baseXrpDrops':base,'grossXrpSeizedDrops':gross,'poolGainDrops':gross-fee,'protocolClaimDrops':fee,'remainingDebtE8s':remaining_debt,'remainingXrpDrops':remaining,'postCR':str(D(remaining)/E6*P/(D(remaining_debt)/E8)) if remaining_debt else None}
a=sized(debt);t=sized(repay);assert a['grossXrpSeizedDrops']==3223825536 and a['poolGainDrops']==3217077995 and a['protocolClaimDrops']==6747541 and a['remainingXrpDrops']==275174474;assert D(t['postCR'])>=R
extra_xrp=D(a['grossXrpSeizedDrops']-t['grossXrpSeizedDrops'])/E6;extra_debt=D(a['debtRetiredE8s']-t['debtRetiredE8s'])/E8;cost=extra_xrp*P-extra_debt
out={'valuationBasis':'protocol $1/icUSD accounting; incident XRP oracle, excluding delivery fees/market changes','oraclePriceUsdPerXrp':str(P),'targetRecoveryCR':str(R),'liquidationBonusMultiplier':str(B),'protocolShareOfBonus':str(F),'reconstructedBefore':{'collateralDrops':C,'debtE8s':debt,'CR':str(D(C)/E6*P/(D(debt)/E8))},'continuousDebtRetirementIcusd':str(continuous),'rounding':'ceil closed-form repayment to icUSD e8s; existing floor collateral conversion, bonus and protocol split; conservative minimal up to native-unit quantization, not a global search','intended':t,'actual':a,'extraXrpSeized':str(extra_xrp),'extraDebtRetiredIcusd':str(extra_debt),'netExcessPenaltyUsd':str(cost),'cashCompensationCeilE6':int((cost*E6).to_integral_value(rounding=ROUND_CEILING)),'xrpEquivalentCompensationCeilDrops':int((cost/P*E6).to_integral_value(rounding=ROUND_CEILING)),'borrowerEffectiveIcusdPerGrossXrp':str(D(debt)/E8/(D(a['grossXrpSeizedDrops'])/E6)),'normalMode117PercentTriggerPrice':str(D(debt)/E8*D('1.17')/(D(C)/E6)),'actualXrpSeizedPercent':str(D(a['grossXrpSeizedDrops'])/D(C)*100),'intendedXrpSeizedPercent':str(D(t['grossXrpSeizedDrops'])/D(C)*100),'executionAuthorized':False}
r=Path('/private/tmp/vault208-investigation-20261115');(r/'comparison-calculation.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out,indent=2))
