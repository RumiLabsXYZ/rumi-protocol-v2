use std::ops::{Deref, DerefMut};
use std::time::SystemTime;

pub struct PocketIc(pocket_ic_v9::PocketIc);

impl Deref for PocketIc {
    type Target = pocket_ic_v9::PocketIc;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PocketIc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl PocketIc {
    pub fn query_call(
        &self,
        canister_id: candid::Principal,
        sender: candid::Principal,
        method: &str,
        payload: Vec<u8>,
    ) -> Result<WasmResult, String> {
        Ok(self
            .0
            .query_call(canister_id, sender, method, payload)
            .map(WasmResult::Reply)
            .unwrap_or_else(|reject| WasmResult::Reject(reject.reject_message)))
    }

    pub fn update_call(
        &self,
        canister_id: candid::Principal,
        sender: candid::Principal,
        method: &str,
        payload: Vec<u8>,
    ) -> Result<WasmResult, String> {
        Ok(self
            .0
            .update_call(canister_id, sender, method, payload)
            .map(WasmResult::Reply)
            .unwrap_or_else(|reject| WasmResult::Reject(reject.reject_message)))
    }

    pub fn get_time(&self) -> SystemTime {
        SystemTime::try_from(self.0.get_time()).expect("PocketIC time is representable")
    }

    pub fn set_time(&self, time: SystemTime) {
        self.0.set_time(time.into());
    }
}

pub struct PocketIcBuilder(pocket_ic_v9::PocketIcBuilder);

impl PocketIcBuilder {
    pub fn new() -> Self {
        Self(pocket_ic_v9::PocketIcBuilder::new())
    }

    pub fn with_nns_subnet(mut self) -> Self {
        self.0 = self.0.with_nns_subnet();
        self
    }

    pub fn build(self) -> PocketIc {
        PocketIc(self.0.build())
    }
}

#[derive(Debug)]
pub enum WasmResult {
    Reply(Vec<u8>),
    Reject(String),
}
