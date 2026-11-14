#![no_main]
// The harness, with what it checks, is `bastion_mc::spotify::picture::fuzz`.
libfuzzer_sys::fuzz_target!(|data: &[u8]| bastion_mc::spotify::picture::fuzz(data));
