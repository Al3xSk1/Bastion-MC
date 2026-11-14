#![no_main]
// The harness, with what it checks, is `bastion_mc::fuzzing::realms`.
libfuzzer_sys::fuzz_target!(|data: &[u8]| bastion_mc::fuzzing::realms(data));
