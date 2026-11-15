use overdrive_core::build_profile;
use overdrive_core::coach::{self, FlowId, StepKind, VerifyStatus};
use overdrive_core::issues::{structural_issues, FixPath, Impact};
use overdrive_core::profile::*;
use overdrive_core::source::{RawMedia, RawPcieLink, RawStabilityEvents};
use overdrive_core::testing::FakeSource;

fn ids(p: &HardwareProfile) -> Vec<String> {
    structural_issues(p).into_iter().map(|i| i.id).collect()
}

#[test]
fn laptop_hybrid_profile() {
    let p = build_profile(&FakeSource::laptop_hybrid());
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert_eq!(p.tier, Tier::Tier2, "4 GB VRAM + 8 GB RAM");
    assert!(p.is_laptop());
    assert_eq!(p.gpus.len(), 2);
    assert_eq!(
        p.integrated_gpu().unwrap().kind.confidence,
        Confidence::Inferred
    );
    assert_eq!(p.memory.channels.val(), Some(1));
    assert_eq!(
        p.memory.channels.confidence,
        Confidence::Inferred,
        "channels are always inferred"
    );
    assert_eq!(
        p.windows.is_windows_11.val(),
        Some(true),
        "build 26100 is Windows 11 even if ProductName says 10"
    );
    assert!(p.windows.name.get().unwrap().starts_with("Windows 11"));
    let issues = ids(&p);
    assert_eq!(issues[0], "single_channel_ram", "{issues:?}");
    assert!(issues.contains(&"hybrid_gpu".into()));
    assert!(issues.contains(&"game_dvr_on".into()));
    assert!(
        !issues.contains(&"ram_below_rated".into()),
        "OEM JEDEC module runs at its speed"
    );
    assert!(!issues.contains(&"uma_small".into()), "has a dedicated GPU");
}

#[test]
fn desktop_entry_xmp_off_on_z490_gets_bios_flow() {
    let p = build_profile(&FakeSource::desktop_entry());
    assert_eq!(p.tier, Tier::Tier2);
    assert_eq!(p.memory.configured_mts.val(), Some(2133));
    assert_eq!(p.memory.rated_mts.val(), Some(3200));
    assert_eq!(
        p.memory.rated_mts.confidence,
        Confidence::Inferred,
        "rated from part number"
    );
    assert_eq!(p.memory.channels.val(), Some(2));
    assert_eq!(p.firmware.chipset.get().map(String::as_str), Some("Z490"));
    assert_eq!(p.memory.platform_cap_mts.val(), None, "Z490 allows XMP");
    let all = structural_issues(&p);
    let ram = all
        .iter()
        .find(|i| i.id == "ram_below_rated")
        .expect("ram issue");
    assert_eq!(ram.impact, Impact::High);
    assert_eq!(
        ram.fixable_by,
        FixPath::Bios {
            flow_id: "ram_xmp".into()
        }
    );
    assert_eq!(ram.params["target"], 3200);
    assert_eq!(ram.params["fallback"], 3000);
    assert!(all.iter().any(|i| i.id == "ssd_nearly_full"));
    assert!(
        all.iter().any(|i| i.id == "gpu_driver_old"),
        "driver from 2023"
    );

    let flow = coach::flow_for(ram, &p).expect("flow");
    assert_eq!(flow.flow_id, FlowId::RamXmp);
    assert_eq!(flow.vendor, BoardVendor::Msi);
    assert_eq!(
        (flow.before, flow.target, flow.fallback),
        (2133, 3200, Some(3000))
    );
    assert!(flow
        .steps
        .iter()
        .any(|s| s.key == "coach.ram_xmp.where.msi"));
    assert!(flow.steps.iter().any(|s| s.kind == StepKind::Reboot));
}

#[test]
fn intel_10th_gen_on_b460_is_locked_not_misadvised() {
    let mut src = FakeSource::desktop_entry();
    src.firmware.board_product = "B460M PRO-VDH WIFI (MS-7C83)".into();
    src.memory.modules.iter_mut().for_each(|m| {
        m.speed_mts = Some(2666);
        m.configured_mts = Some(2666);
    });
    let p = build_profile(&src);
    assert_eq!(p.memory.platform_cap_mts.val(), Some(2666));
    let issues = ids(&p);
    assert!(
        !issues.contains(&"ram_below_rated".into()),
        "can't go above 2666 on B460 with an i3: {issues:?}"
    );
    assert!(issues.contains(&"ram_platform_locked".into()));

    // Same board, RAM at 2133: target is the cap (2666), not the kit's 3200.
    src.memory
        .modules
        .iter_mut()
        .for_each(|m| m.configured_mts = Some(2133));
    let p = build_profile(&src);
    let ram = structural_issues(&p)
        .into_iter()
        .find(|i| i.id == "ram_below_rated")
        .unwrap();
    assert_eq!(ram.params["target"], 2666);
}

#[test]
fn apu_lowend_tier1_with_uma_and_hdd() {
    let p = build_profile(&FakeSource::apu_lowend());
    assert_eq!(p.tier, Tier::Tier1);
    let all = structural_issues(&p);
    let order: Vec<&str> = all.iter().map(|i| i.id.as_str()).collect();
    assert!(order.contains(&"system_on_hdd"));
    assert!(order.contains(&"ram_below_rated"));
    let uma = all.iter().find(|i| i.id == "uma_small").unwrap();
    assert_eq!(
        uma.fixable_by,
        FixPath::Bios {
            flow_id: "uma_buffer".into()
        }
    );
    let flow = coach::flow_for(uma, &p).unwrap();
    assert_eq!(flow.target, 1024, "only ~8 GB RAM: be conservative");
    assert!(flow
        .steps
        .iter()
        .any(|s| s.key == "coach.uma_buffer.where.asus"));
    // Ranking: all High before Medium before Low
    let impacts: Vec<Impact> = all.iter().map(|i| i.impact).collect();
    let mut sorted = impacts.clone();
    sorted.sort();
    assert_eq!(impacts, sorted);
}

#[test]
fn laptop_with_slow_ram_is_not_sent_to_the_bios() {
    let mut src = FakeSource::laptop_hybrid();
    src.memory.modules[0].part_number = "KF432S20IB/8".into(); // Fury Impact 3200 (SO-DIMM)
    src.memory.modules[0].configured_mts = Some(2666);
    src.memory.modules[0].speed_mts = Some(2666);
    let p = build_profile(&src);
    let ram = structural_issues(&p)
        .into_iter()
        .find(|i| i.id == "ram_below_rated")
        .unwrap();
    assert_eq!(ram.fixable_by, FixPath::User);
    assert_eq!(ram.params["laptop"], true);
}

#[test]
fn failing_sources_degrade_to_unknown_not_errors() {
    let mut src = FakeSource::desktop_entry();
    src.failing = ["memory", "gpus", "perf_sample", "firmware"]
        .into_iter()
        .collect();
    let p = build_profile(&src);
    assert_eq!(p.warnings.len(), 4, "{:?}", p.warnings);
    assert_eq!(p.memory.configured_mts.confidence, Confidence::Unknown);
    assert!(p.gpus.is_empty());
    assert_eq!(p.cpu.perf_limit_pct.confidence, Confidence::Unknown);
    assert_eq!(p.cpu.cores.val(), Some(4), "other sources still work");
    // Serializes cleanly with nulls for unknowns.
    let json = serde_json::to_value(&p).unwrap();
    assert!(json["memory"]["configuredMts"]["value"].is_null());
    assert_eq!(json["memory"]["configuredMts"]["confidence"], "unknown");
}

#[test]
fn hybrid_cpu_core_counts() {
    let mut src = FakeSource::desktop_entry();
    src.cpu.name = "13th Gen Intel(R) Core(TM) i5-13400F".into();
    src.cpu.cores = 10;
    src.cpu.efficiency_classes = Some(vec![1, 1, 1, 1, 1, 1, 0, 0, 0, 0]);
    let p = build_profile(&src);
    assert_eq!(p.cpu.performance_cores.val(), Some(6));
    assert_eq!(p.cpu.efficiency_cores.val(), Some(4));
    assert_eq!(p.cpu.generation.val(), Some(13));
}

#[test]
fn pcie_width_and_throttling_signals() {
    let mut src = FakeSource::desktop_entry();
    src.gpus[0].pcie = Some(RawPcieLink {
        current_width: Some(4),
        max_width: Some(16),
        current_gen: Some(3),
        max_gen: Some(3),
    });
    src.perf.perf_limit_pct = Some(62.0);
    let issues = ids(&build_profile(&src));
    assert!(issues.contains(&"pcie_width_reduced".into()));
    assert!(issues.contains(&"cpu_throttling".into()));
}

#[test]
fn system_drive_detection_uses_letter() {
    let mut src = FakeSource::apu_lowend();
    src.drives[0].volumes[0].letter = "D:".into();
    let p = build_profile(&src);
    assert!(p.system_drive().is_none());
    assert!(!ids(&p).contains(&"system_on_hdd".into()));
    src.drives[0].media = RawMedia::Ssd;
    src.drives[0].volumes[0].letter = "c:".into();
    assert!(build_profile(&src).system_drive().is_some());
}

#[test]
fn coach_verification_and_stability() {
    let src = FakeSource::desktop_entry();
    let before = build_profile(&src);
    let issue = structural_issues(&before)
        .into_iter()
        .find(|i| i.id == "ram_below_rated")
        .unwrap();
    let flow = coach::flow_for(&issue, &before).unwrap();

    let mut after_src = src.clone();
    after_src
        .memory
        .modules
        .iter_mut()
        .for_each(|m| m.configured_mts = Some(3200));
    let v = coach::verify_after_reboot(&flow, &build_profile(&after_src));
    assert_eq!(v.status, VerifyStatus::Applied);
    assert_eq!(v.after, Some(3200));
    assert!(
        !ids(&build_profile(&after_src)).contains(&"ram_below_rated".into()),
        "issue resolved"
    );

    after_src
        .memory
        .modules
        .iter_mut()
        .for_each(|m| m.configured_mts = Some(2933));
    assert_eq!(
        coach::verify_after_reboot(&flow, &build_profile(&after_src)).status,
        VerifyStatus::Partial
    );
    assert_eq!(
        coach::verify_after_reboot(&flow, &before).status,
        VerifyStatus::NotApplied
    );

    let ok = coach::assess_stability(&flow, RawStabilityEvents::default());
    assert!(ok.stable);
    let bad = coach::assess_stability(
        &flow,
        RawStabilityEvents {
            whea_errors: 2,
            ..Default::default()
        },
    );
    assert!(!bad.stable);
    assert_eq!(bad.advice_key, "coach.unstable.step_down");
    assert_eq!(bad.fallback, Some(3000));
}

#[test]
fn profile_roundtrips_through_json() {
    let p = build_profile(&FakeSource::laptop_hybrid());
    let s = serde_json::to_string(&p).unwrap();
    let back: HardwareProfile = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back);
}
