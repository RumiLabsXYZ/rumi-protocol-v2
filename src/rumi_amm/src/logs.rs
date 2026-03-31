use ic_canister_log::{declare_log_buffer, GlobalBuffer, Sink};

declare_log_buffer!(name = INFO_BUF, capacity = 1000);

pub const INFO: PrintProxySink = PrintProxySink("INFO", &INFO_BUF);

pub struct PrintProxySink(&'static str, &'static GlobalBuffer);

impl Sink for PrintProxySink {
    fn append(&self, entry: ic_canister_log::LogEntry) {
        ic_cdk::println!("{} {}:{} {}", self.0, entry.file, entry.line, entry.message);
        self.1.append(entry)
    }
}
