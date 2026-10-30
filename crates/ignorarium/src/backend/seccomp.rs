//! The seccomp filter every level 1 sandbox runs under. It refuses io_uring and
//! `AF_VSOCK` sockets with `EPERM` and allows every other call. `socketcall` exists only
//! in the 32-bit ABIs, and the filter's architecture check kills any call made through a
//! foreign ABI.
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};
use std::collections::BTreeMap;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the level 1 seccomp filter is built for x86_64 and aarch64 only");

#[cfg(target_arch = "x86_64")]
const ARCH: TargetArch = TargetArch::x86_64;
#[cfg(target_arch = "aarch64")]
const ARCH: TargetArch = TargetArch::aarch64;

/// Bits added to each call number. An x86_64 kernel built with the x32 ABI also takes
/// each call under the same architecture tag with bit 30 set.
#[cfg(target_arch = "x86_64")]
const ABIS: [i64; 2] = [0, 0x4000_0000];
#[cfg(target_arch = "aarch64")]
const ABIS: [i64; 1] = [0];

/// The compiled filter as raw `sock_filter` records (code u16, jt u8, jf u8, k u32, native
/// endian), the form bubblewrap reads from `--seccomp`. Empty if the rules fail to
/// compile; bubblewrap then refuses to start the sandbox.
pub fn program() -> Vec<u8> {
    compile()
        .map(|p| {
            p.iter()
                .flat_map(|f| {
                    let [c0, c1] = f.code.to_ne_bytes();
                    let [k0, k1, k2, k3] = f.k.to_ne_bytes();
                    [c0, c1, f.jt, f.jf, k0, k1, k2, k3]
                })
                .collect()
        })
        .unwrap_or_default()
}

fn compile() -> Result<BpfProgram, seccompiler::BackendError> {
    let vsock = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Eq,
        libc::AF_VSOCK as u64,
    )?;
    let denied = [
        (libc::SYS_io_uring_setup, vec![]),
        (libc::SYS_io_uring_enter, vec![]),
        (libc::SYS_io_uring_register, vec![]),
        (libc::SYS_socket, vec![SeccompRule::new(vec![vsock])?]),
    ];
    let rules: BTreeMap<i64, Vec<SeccompRule>> = ABIS
        .iter()
        .flat_map(|abi| denied.iter().map(move |(nr, r)| (nr | abi, r.clone())))
        .collect();
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        ARCH,
    )?;
    BpfProgram::try_from(filter)
}
