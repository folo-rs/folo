//! Smoke test for the public re-export contract.
//!
//! `many_cpus` is a thin shell over `many_cpus_impl` and exposes a hand-picked subset of items.
//! This test fails to compile if any of those re-exports go missing, which keeps the
//! intended public surface honest as `many_cpus_impl` evolves. It does not exercise behavior
//! beyond what is needed to verify that each type is reachable from outside `many_cpus`.

use std::panic::{RefUnwindSafe, UnwindSafe};

use many_cpus::{
    EfficiencyClass, MemoryRegionId, Processor, ProcessorId, ProcessorSet, ProcessorSetBuilder,
    RelativeSpeed, ResourceQuota, SystemHardware,
};
use static_assertions::assert_impl_all;

assert_impl_all!(SystemHardware: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(ProcessorSet: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(ProcessorSetBuilder: Send, Sync);
assert_impl_all!(Processor: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(ResourceQuota: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(RelativeSpeed: Send, Sync, UnwindSafe, RefUnwindSafe);

fn require_processor_id(_: ProcessorId) {}
fn require_memory_region_id(_: MemoryRegionId) {}
fn require_efficiency_class(_: EfficiencyClass) {}
fn require_relative_speed(_: RelativeSpeed) {}

#[test]
fn system_hardware_inspection_via_re_exports() {
    let hardware = SystemHardware::current();

    _ = hardware.max_processor_count();
    _ = hardware.max_memory_region_count();

    require_processor_id(hardware.current_processor_id());
    require_memory_region_id(hardware.current_memory_region_id());
}

#[test]
fn processor_set_basics_via_re_exports() {
    let hardware = SystemHardware::current();
    let processors: ProcessorSet = hardware.processors();
    assert!(processors.len() >= 1);

    let any_processor: &Processor = processors.processors().first();
    require_processor_id(any_processor.id());
    require_memory_region_id(any_processor.memory_region_id());
    require_efficiency_class(any_processor.efficiency_class());
    require_relative_speed(any_processor.relative_speed());
}

#[test]
fn processor_set_builder_basics_via_re_exports() {
    let hardware = SystemHardware::current();
    let builder: ProcessorSetBuilder = hardware.processors().to_builder();
    _ = builder.take_all();
}

#[cfg(feature = "test-util")]
mod with_test_util {
    use many_cpus::SystemHardware;
    use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    assert_impl_all!(HardwareBuilder: Send);
    assert_impl_all!(ProcessorBuilder: Send);

    #[test]
    fn fake_hardware_constructor_via_re_exports() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(2));
        let hardware = SystemHardware::fake(builder);
        assert!(hardware.max_processor_count() >= 4);
    }

    #[test]
    fn fake_processor_builder_constructor_via_re_exports() {
        let builder = HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(0).relative_speed(nz!(3600)))
            .processor(ProcessorBuilder::new().id(1).memory_region(1));
        let hardware = SystemHardware::fake(builder);
        assert!(hardware.max_processor_count() >= 2);
    }
}
