//! Reduces needless prompts: common commands with no write form (git --version, git --no-pager reads, sleep, timeout <duration> <read>) count as reads, with nothing missed.
// Value: protects=common read-only commands (git --version, git --no-pager <read>, sleep, timeout <duration> <read>) raise no permission prompt;
//   fails_when=a delete, write or unknown command after timeout or --no-pager is passed as a read, or the deliberately excluded git -C and -c are opened up;
//   why_new=top false-ask sources observed in replay (git, sleep and timeout forms); seam=classify::assess_bash (public); no_claim=false-ask counts for commands outside the replay.
use olp_hook::classify::{assess_bash, Kind, Limits};
use olp_hook::store::DeviceMap;
use std::path::Path;

fn read(cmd: &str) -> bool {
    assess_bash(cmd, &DeviceMap::default(), Path::new("."), Limits { counter: None }).is_read()
}

#[test]
fn git_version_and_no_pager_reads_are_reads() {
    for cmd in ["git --version", "git --no-pager log -1 --oneline", "git --no-pager diff --stat", "git --no-pager show HEAD:README.md"] {
        assert!(read(cmd), "{cmd}: should be a read");
    }
    // What follows is not a read: stays as is
    for cmd in ["git --no-pager push", "git --no-pager commit -m x", "git --version && git push"] {
        assert!(!read(cmd), "{cmd}: must not be a read");
    }
}

#[test]
fn git_global_options_that_can_launch_programs_stay_unread() {
    // Keep the deliberate exclusions: -C, -c and --exec-path can launch programs through configuration
    for cmd in ["git -C /tmp/x status", "git -c core.pager=less log", "git --exec-path=/x status", "git --no-pager -c core.fsmonitor=x status"] {
        assert!(!read(cmd), "{cmd}: must stay not a read");
    }
}

#[test]
fn sleep_is_a_read_and_keeps_loops_readable() {
    for cmd in ["sleep 5", "sleep 5; tail -n 3 out.log", "until grep -q DONE out.log; do sleep 20; done; tail -n 5 out.log"] {
        assert!(read(cmd), "{cmd}: should be a read");
    }
}

#[test]
fn timeout_wraps_the_command_after_its_duration() {
    for cmd in ["timeout 30 tail -n 5 out.log", "timeout -k 5 30 grep -c x out.log", "timeout --signal=KILL 10s cat a.txt", "timeout -s KILL 10 head -3 a.txt"] {
        assert!(read(cmd), "{cmd}: wrapped command is a read, so a read");
    }
    for cmd in ["timeout 30 deno test", "timeout 30 cargo build", "timeout 30"] {
        assert!(!read(cmd), "{cmd}: must not be a read");
    }
    // Nothing missed: a delete after timeout is still a delete prompt
    let a = assess_bash("timeout 30 rm -rf build", &DeviceMap::default(), Path::new("."), Limits { counter: None });
    assert!(!a.is_read() && a.findings.iter().any(|f| f.kind == Kind::FileDelete), "timeout 30 rm -rf build must be a delete");
}
