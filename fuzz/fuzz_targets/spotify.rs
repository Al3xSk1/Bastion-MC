#![no_main]
// The harness, with what it checks, is `bastion_mc::spotify::web::fuzz`.
libfuzzer_sys::fuzz_target!(|data: &[u8]| bastion_mc::spotify::web::fuzz(data));
