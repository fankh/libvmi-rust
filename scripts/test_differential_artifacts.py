import os
import random
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path


WORKSPACE = Path(__file__).resolve().parents[1]


def cli_path():
    suffix = ".exe" if os.name == "nt" else ""
    return WORKSPACE / "target" / "debug" / f"vmi-cli{suffix}"


def decode_cli(output):
    decoded = bytearray()
    for line in output.splitlines():
        _, values = line.split(":", 1)
        decoded.extend(int(value, 16) for value in values.split())
    return bytes(decoded)


def run_read(command, path, address, length):
    completed = subprocess.run(
        [cli_path(), command, path, hex(address), str(length)],
        cwd=WORKSPACE,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode:
        raise AssertionError(completed.stderr)
    return decode_cli(completed.stdout)


def elf64(segments):
    header_size = 64
    program_header_size = 56
    data_offset = 0x1000
    image = bytearray(data_offset)
    ident = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    image[:header_size] = struct.pack(
        "<16sHHIQQQIHHHHHH",
        ident,
        4,
        62,
        1,
        0,
        header_size,
        0,
        0,
        header_size,
        program_header_size,
        len(segments),
        0,
        0,
        0,
    )
    cursor = data_offset
    for index, (physical, data, memory_size) in enumerate(segments):
        program_header = struct.pack(
            "<IIQQQQQQ", 1, 0, cursor, 0, physical, len(data), memory_size, 4096
        )
        start = header_size + index * program_header_size
        image[start : start + program_header_size] = program_header
        image.extend(data)
        cursor += len(data)
    return bytes(image)


class DifferentialArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run(
            ["cargo", "build", "--quiet", "-p", "vmi-cli"],
            cwd=WORKSPACE,
            check=True,
        )

    def test_raw_reads_match_python_slice_oracle(self):
        randomizer = random.Random(0x564D49)
        content = bytes(randomizer.randrange(256) for _ in range(1024))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "memory.raw"
            path.write_bytes(content)
            for _ in range(32):
                start = randomizer.randrange(len(content))
                length = randomizer.randrange(len(content) - start + 1)
                self.assertEqual(
                    run_read("read-raw", path, start, length),
                    content[start : start + length],
                )

    def test_elf_reads_match_independent_segment_oracle(self):
        randomizer = random.Random(0x454C46)
        first = bytes(randomizer.randrange(256) for _ in range(16))
        second = bytes(randomizer.randrange(256) for _ in range(32))
        base = 0x1000
        expected = first + bytes(16) + second
        image = elf64([(base, first, 32), (base + 32, second, 32)])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "memory.elf"
            path.write_bytes(image)
            for _ in range(32):
                offset = randomizer.randrange(len(expected))
                length = randomizer.randrange(len(expected) - offset + 1)
                self.assertEqual(
                    run_read("read-elf", path, base + offset, length),
                    expected[offset : offset + length],
                )


if __name__ == "__main__":
    unittest.main()
