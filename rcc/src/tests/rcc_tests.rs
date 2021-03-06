use crate::code_gen::TargetPlatform;
use crate::rcc::{OptimizeLevel, RcCompiler, RccError};
use std::io::Read;

fn file_path(file_name: &str) -> String {
    format!("./src/tests/{}", file_name)
}

fn test_compile(input: &str, expected_output: &str) -> Result<(), RccError> {
    let input = std::fs::File::open(file_path(input))?;
    let output = Vec::<u8>::new();
    let mut rcc = RcCompiler::new(TargetPlatform::Riscv32, input, output, OptimizeLevel::Zero);

    rcc.compile()?;

    let s = std::str::from_utf8(rcc.output.buffer()).unwrap();
    let mut expected = String::new();
    let mut expected_output = std::fs::File::open(file_path(expected_output))?;
    expected_output.read_to_string(&mut expected)?;
    assert_eq!(expected, s);
    Ok(())
}

#[test]
fn rcc_test_ok() {
    for i in 1..=5 {
        test_compile(&format!("in{}.txt", i), &format!("out{}.txt", i)).unwrap();
    }
}

#[test]
fn rcc_test_error() {
    let errors: [Result<(), RccError>; 1] = [Err("`a_5` may not have definition".into())];
    for (i, err) in (6..=6).zip(errors) {
        assert_eq!(test_compile(&format!("in{}.txt", i), ""), err);
    }
}
