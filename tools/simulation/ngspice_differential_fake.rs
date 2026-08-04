use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments == ["-n", "--version"] {
        require_fixed_environment();
        require_process_limits();
        println!("******");
        println!("** ngspice-45.2 : Circuit level simulation program");
        println!("******");
        return;
    }
    if arguments != ["-n", "-b", "input.spice"] {
        fail("unexpected analysis argv");
    }
    require_fixed_environment();
    require_process_limits();
    let netlist = fs::read_to_string("input.spice").unwrap_or_else(|_| fail("missing input"));
    let (original, control) = netlist
        .split_once(".control\n")
        .unwrap_or_else(|| fail("missing static control instrumentation"));
    if control.contains(".control\n") {
        fail("duplicate control instrumentation");
    }
    let raw = if original.lines().any(|line| line == ".OP") {
        require_control(control, false);
        dc_raw()
    } else if original.lines().any(|line| line.starts_with(".AC LIN ")) {
        require_control(control, false);
        ac_raw(!Path::new("pass-mode").exists())
    } else if original.lines().any(|line| line.starts_with(".TRAN ")) {
        require_control(control, true);
        transient_raw()
    } else {
        fail("missing analysis directive");
    };
    fs::write("result.raw", raw).unwrap_or_else(|_| fail("could not write result"));
}

fn require_control(control: &str, transient: bool) {
    let expected = if transient {
        "set filetype=ascii\nrun\nlinearize\nwrite result.raw\nquit\n.endc\n.END\n"
    } else {
        "set filetype=ascii\nrun\nwrite result.raw\nquit\n.endc\n.END\n"
    };
    if control != expected {
        fail("control instrumentation is not the exact static program");
    }
}

fn require_fixed_environment() {
    let environment: BTreeMap<_, _> = env::vars().collect();
    for (name, value) in [
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("TZ", "UTC"),
        ("OMP_NUM_THREADS", "1"),
        ("OPENBLAS_NUM_THREADS", "1"),
    ] {
        if environment.get(name).map(String::as_str) != Some(value) {
            fail("missing fixed environment");
        }
    }
    if !environment
        .get("HOME")
        .is_some_and(|value| value.ends_with("/home"))
        || !environment
            .get("TMPDIR")
            .is_some_and(|value| value.ends_with("/tmp"))
    {
        fail("missing private HOME or TMPDIR");
    }
    let allowed = BTreeMap::from([
        ("HOME", ()),
        ("LANG", ()),
        ("LC_ALL", ()),
        ("OMP_NUM_THREADS", ()),
        ("OPENBLAS_NUM_THREADS", ()),
        ("TMPDIR", ()),
        ("TZ", ()),
    ]);
    if environment
        .keys()
        .any(|name| !allowed.contains_key(name.as_str()))
    {
        fail("host environment leaked into ngspice");
    }
}

#[cfg(unix)]
fn require_process_limits() {
    if unsafe { libc::getpgrp() } != unsafe { libc::getpid() } {
        fail("ngspice is not its process-group leader");
    }
    for (resource, expected, name) in [
        (libc::RLIMIT_CPU, 6, "CPU"),
        (libc::RLIMIT_FSIZE, 16 << 20, "file-size"),
        (libc::RLIMIT_NOFILE, 64, "descriptor"),
        (libc::RLIMIT_CORE, 0, "core"),
    ] {
        require_limit(resource, expected, name);
    }
    #[cfg(target_os = "linux")]
    require_limit(libc::RLIMIT_AS, 2 << 30, "address-space");
}

#[cfg(not(unix))]
fn require_process_limits() {}

#[cfg(all(unix, target_os = "linux"))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn require_limit(resource: RlimitResource, expected: u64, name: &str) {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` points to initialized writable storage and `resource` is
    // one of the platform RLIMIT constants listed above.
    if unsafe { libc::getrlimit(resource, &mut limit) } != 0
        || limit.rlim_cur != expected as libc::rlim_t
        || limit.rlim_max != expected as libc::rlim_t
    {
        fail(&format!("unexpected {name} resource limit"));
    }
}

fn dc_raw() -> String {
    "Title: fake differential fixture\n\
     Date: deliberately ignored\n\
     Command: deliberately ignored\n\
     Plotname: Operating Point\n\
     Flags: real\n\
     No. Variables: 2\n\
     No. Points: 1\n\
     Variables:\n\
     \t0\tv(vin)\tvoltage\n\
     \t1\tv(vout)\tvoltage\n\
     Values:\n\
     0\t\t1.000000000000000e+01\n\
     \t5.000000000000000e+00\n"
        .to_owned()
}

fn ac_raw(perturb_late_sample: bool) -> String {
    let mut output = String::from(
        "Title: fake differential fixture\n\
         Date: deliberately ignored\n\
         Command: deliberately ignored\n\
         Plotname: AC Analysis\n\
         Flags: complex\n\
         No. Variables: 3\n\
         No. Points: 4\n\
         Variables:\n\
         \t0\tfrequency\tfrequency\n\
         \t1\tv(vin)\tvoltage\n\
         \t2\tv(vout)\tvoltage\n\
         Values:\n",
    );
    for index in 0..4 {
        let frequency = index + 1;
        let vout = if index == 3 && perturb_late_sample {
            "5.000020000000000e-01"
        } else {
            "5.000000000000000e-01"
        };
        output.push_str(&format!(
            "{index}\t\t{frequency}.000000000000000e+00,4.000000000000000e-314\n\
             \t1.000000000000000e+00,0.000000000000000e+00\n\
             \t{vout},0.000000000000000e+00\n"
        ));
    }
    output
}

fn transient_raw() -> String {
    let mut output = String::from(
        "Title: fake differential fixture\n\
         Date: deliberately ignored\n\
         Command: deliberately ignored\n\
         Plotname: Transient Analysis (linearized)\n\
         Flags: real\n\
         No. Variables: 3\n\
         No. Points: 5\n\
         Variables:\n\
         \t0\ttime\ttime\n\
         \t1\tv(vin)\tvoltage\n\
         \t2\tv(vout)\tvoltage\n\
         Values:\n",
    );
    for (index, time) in [
        "0.000000000000000e+00",
        "1.250000000000000e-01",
        "2.500000000000000e-01",
        "3.750000000000000e-01",
        "5.000000000000000e-01",
    ]
    .into_iter()
    .enumerate()
    {
        output.push_str(&format!(
            "{index}\t\t{time}\n\
             \t1.000000000000000e+01\n\
             \t5.000000000000000e+00\n"
        ));
    }
    output
}

fn fail(message: &str) -> ! {
    eprintln!("fake ngspice contract failure: {message}");
    std::process::exit(2);
}
