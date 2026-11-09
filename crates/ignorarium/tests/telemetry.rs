use ignorarium::canon::sha256_hex;
use ignorarium::telemetry::{CpuTicks, HostCpu, Reading, cpu_ticks_of, host_cpu_of, record};
use serde_json::json;

const STAT: &str = "cpu  10132153 290696 3084719 46828483 16683 1 25195 175628 7 3
cpu0 5066076 145348 1542359 23414241 8341 0 12597 87814 3 1
intr 1234 5 6
ctxt 99
";

#[test]
fn cpu_ticks_of_reads_the_aggregate_line() {
    let want = vec![
        10_132_153, 290_696, 3_084_719, 46_828_483, 16_683, 1, 25_195, 175_628, 7, 3,
    ];
    assert_eq!(
        cpu_ticks_of(STAT, 100),
        Reading::Read(CpuTicks {
            user_hz: 100,
            ticks: want
        })
    );
    // Only per-CPU lines: no aggregate line.
    let per_cpu = "cpu0 1 2 3 4 5 6 7 8 9 10\ncpu1 1 2 3 4 5 6 7 8 9 10\n";
    assert!(matches!(
        cpu_ticks_of(per_cpu, 100),
        Reading::Unavailable(_)
    ));
    assert!(matches!(cpu_ticks_of("", 100), Reading::Unavailable(_)));
    // An old kernel gives 7 fields.
    assert_eq!(
        cpu_ticks_of("cpu  1 2 3 4 5 6 7\ncpu0 1 2 3 4 5 6 7\n", 250),
        Reading::Read(CpuTicks {
            user_hz: 250,
            ticks: vec![1, 2, 3, 4, 5, 6, 7]
        })
    );
    // A field that is no number is unavailable, not a zero.
    assert!(matches!(
        cpu_ticks_of("cpu  1 2 x 4\n", 100),
        Reading::Unavailable(_)
    ));
}

const X86: &str = "processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel Core Processor (Haswell, no TSX)
flags\t\t: fpu vme de pse tsc
bugs\t\t: cpu_meltdown

processor\t: 1
model name\t: Another Model
flags\t\t: other flags
";

const AARCH64: &str = "processor\t: 0
BogoMIPS\t: 50.00
Features\t: fp asimd evtstrm
CPU implementer\t: 0x41
";

#[test]
fn host_cpu_of_reads_model_and_hashes_flags() {
    assert_eq!(
        host_cpu_of(X86),
        Reading::Read(HostCpu {
            model: "Intel Core Processor (Haswell, no TSX)".into(),
            flags_sha256: sha256_hex(b"fpu vme de pse tsc"),
        })
    );
    assert_eq!(
        host_cpu_of(AARCH64),
        Reading::Unavailable("/proc/cpuinfo names no model name".into())
    );
    // A model with no flags cannot be fingerprinted.
    assert!(matches!(
        host_cpu_of("model name\t: M\n"),
        Reading::Unavailable(_)
    ));
}

#[test]
fn a_reading_serializes_as_its_value_or_as_its_reason() {
    let read = Reading::Read(vec![1, 2]);
    assert_eq!(serde_json::to_value(&read).unwrap(), json!([1, 2]));
    let gone: Reading<u8> = Reading::Unavailable("why".into());
    assert_eq!(
        serde_json::to_value(&gone).unwrap(),
        json!({ "unavailable": "why" })
    );
}

#[test]
fn the_record_holds_both_tick_reads_and_redacts_reasons() {
    let at = |n: u64| {
        Reading::Read(CpuTicks {
            user_hz: 100,
            ticks: vec![n, n + 1],
        })
    };
    let host = Reading::Read(HostCpu {
        model: "m".into(),
        flags_sha256: "f".into(),
    });
    let keep = |s: &str| s.to_string();
    let v = record(&host, &at(5), &at(9), &keep);
    assert_eq!(
        v,
        json!({
            "host_cpu": { "model": "m", "flags_sha256": "f" },
            "cpu_ticks": { "user_hz": 100, "start": [5, 6], "end": [9, 10] },
            "cgroup": { "unavailable": "not read yet" },
        })
    );
    // One failed read makes the whole pair unavailable; every reason is redacted.
    let gone: Reading<CpuTicks> = Reading::Unavailable("read /srv/x failed".into());
    let hide = |s: &str| s.replace("/srv/x", "<home>");
    let none: Reading<HostCpu> = Reading::Unavailable("no /srv/x".into());
    let v = record(&none, &at(5), &gone, &hide);
    assert_eq!(
        v["cpu_ticks"],
        json!({ "unavailable": "read <home> failed" })
    );
    assert_eq!(v["host_cpu"], json!({ "unavailable": "no <home>" }));
    let v = record(&host, &gone, &at(5), &hide);
    assert_eq!(
        v["cpu_ticks"],
        json!({ "unavailable": "read <home> failed" })
    );
}
