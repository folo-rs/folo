//! Real hardware acquisition through the public environment-probe API.

#![cfg(not(miri))]

use std::collections::BTreeSet;
use std::task::{Context, Poll, Waker};

use cbh_probe::{EnvironmentProbe, HardwareProfile, SystemProbe, resolve_machine_key};
use many_cpus::{Processor, SystemHardware};

::testing::set_allocator!();

/// Width of the hardware fingerprint's stored hexadecimal representation.
const FINGERPRINT_HEX_LEN: usize = 16;

fn system_profile() -> HardwareProfile {
    let probe = SystemProbe::default();
    // Hardware acquisition completes before this adapter returns its ready future;
    // polling it directly avoids adding a runtime merely to retrieve that result.
    let mut profile = Box::pin(probe.hardware());
    match profile
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(profile) => profile,
        Poll::Pending => panic!("the synchronous hardware probe must return a ready result"),
    }
}

#[test]
fn system_profile_reports_at_least_one_processor() {
    let hardware = system_profile();
    assert!(hardware.processors >= 1, "{hardware:?}");
    assert!(hardware.memory_regions >= 1, "{hardware:?}");
    // The fingerprint of whatever this machine is must be well-formed.
    assert_eq!(resolve_machine_key(&hardware).len(), FINGERPRINT_HEX_LEN);
}

#[test]
fn system_profile_counts_usable_hardware_rather_than_the_id_space() {
    // The ID space may include reserved or offline processors; usable counts must
    // describe the hardware on which this process can actually execute.
    let hardware = SystemHardware::current();
    let usable = hardware.all_processors();
    let profile = system_profile();

    assert_eq!(profile.processors, usable.len(), "{profile:?}");
    assert!(
        profile.processors <= hardware.max_processor_count(),
        "{profile:?}"
    );

    let usable_regions = usable
        .iter()
        .map(Processor::memory_region_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(profile.memory_regions, usable_regions.len(), "{profile:?}");
    assert!(
        profile.memory_regions <= hardware.max_memory_region_count(),
        "{profile:?}"
    );
}
