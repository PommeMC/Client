use crate::memory::MemoryStats;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;

const fn mib(b: u64) -> u64 {
    b / 1024 / 1024
}

pub struct MemoryDebugEntry;

impl DebugEntry for MemoryDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, _info: &DebugInfo) {
        let m = MemoryStats::sample();
        let percent = (m.live_bytes * 100).checked_div(m.max).unwrap_or(0);

        displayer.add_fact_to_group(DebugGroup::Memory, "Used", |f| {
            f.value(format!("{percent:>2}"))
                .text("% ")
                .value(format!("{:03}", mib(m.live_bytes)))
                .text("/")
                .value(format!("{:03}", mib(m.max)))
                .text("MiB");
        });
    }
}

pub struct DetailedMemoryDebugEntry;

impl DebugEntry for DetailedMemoryDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, _info: &DebugInfo) {
        let m = MemoryStats::sample();

        displayer.add_fact_to_group(DebugGroup::Memory, "Allocs", |f| {
            f.value(m.live_allocs);
        });
        displayer.add_fact_to_group(DebugGroup::Memory, "Alloc rate", |f| {
            f.value(format!("{:03}", mib(m.alloc_bytes_per_sec)))
                .text("MiB/s");
        });
        displayer.add_fact_to_group(DebugGroup::Memory, "Dealloc rate", |f| {
            f.value(format!("{:03}", mib(m.free_bytes_per_sec)))
                .text("MiB/s");
        });
    }
}
