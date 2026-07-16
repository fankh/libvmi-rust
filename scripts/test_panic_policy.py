#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-panic-policy.py")
SPEC = importlib.util.spec_from_file_location("verify_panic_policy", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class PanicPolicyValidatorTests(unittest.TestCase):
    def validate(self, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "lib.rs"
            path.write_text(source, encoding="utf-8")
            return validator.validate_source(path)

    def test_fallible_production_code_passes(self) -> None:
        self.assertEqual(self.validate("fn value() -> Option<u8> { Some(1) }\n"), [])

    def test_each_production_panic_primitive_fails(self) -> None:
        source = """
fn bad(value: Option<u8>, result: Result<u8, ()>) {
    value.unwrap();
    result.expect("value");
    panic!("bad");
    unreachable!("bad");
    todo!("bad");
    unimplemented!("bad");
}
"""
        self.assertEqual(len(self.validate(source)), 6)

    def test_trailing_test_module_may_assert_with_unwrap(self) -> None:
        source = """
fn value() -> Option<u8> { Some(1) }
#[cfg(test)]
mod tests {
    #[test]
    fn works() { assert_eq!(super::value().unwrap(), 1); }
}
"""
        self.assertEqual(self.validate(source), [])


if __name__ == "__main__":
    unittest.main()
