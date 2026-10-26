# /// script
# requires-python = ">=3.10"
# dependencies = ["hidapi>=0.14.0"]
# ///
r"""Read-only Logitech HID++ 2.0 diagnostic for Windows + uv.

Examples:
    uv run .\\hidpp_debug.py
    uv run .\hidpp_debug.py --pid 0xC547
    uv run .\hidpp_debug.py --pid 0xC547 --slots 1

The script only sends HID++ GetFeature requests and battery status read requests.
It does not alter settings, pairing, or firmware.
"""
from __future__ import annotations

import argparse
import datetime as _dt
import os
import sys
import time
from pathlib import Path
from typing import Any

try:
    import hid  # provided by the `hidapi` PyPI package
except ImportError:
    print("ERROR: Python package 'hidapi' is missing. Run this file with: uv run .\\hidpp_debug.py", file=sys.stderr)
    raise

HIDPP_USAGE_PAGE = 0xFF00
SHORT_USAGE = 0x0001
LONG_USAGE = 0x0002
SHORT_REPORT_ID = 0x10
LONG_REPORT_ID = 0x11
SHORT_REPORT_LEN = 7
LONG_REPORT_LEN = 20
ROOT_FEATURE_INDEX = 0x00
DEFAULT_VID = 0x046D
DEFAULT_PID = 0xC547
DEFAULT_TIMEOUT_MS = 350

FEATURE_NAMES = {
    0x0003: "Device Information",
    0x1000: "Battery Status (legacy on many devices)",
    0x1001: "Battery Voltage",
    0x1004: "Unified Battery (common on newer devices)",
}


class Log:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.file = path.open("w", encoding="utf-8", newline="\n")

    def write(self, message: str = "") -> None:
        stamp = _dt.datetime.now().astimezone().strftime("%H:%M:%S.%f")[:-3]
        line = f"[{stamp}] {message}" if message else ""
        print(line, flush=True)
        self.file.write(line + "\n")
        self.file.flush()

    def close(self) -> None:
        self.file.close()


def parse_int(text: str) -> int:
    return int(text, 0)


def parse_slots(text: str) -> list[int]:
    result: list[int] = []
    for part in text.split(","):
        part = part.strip().lower()
        if not part:
            continue
        value = 0xFF if part in {"ff", "0xff", "receiver"} else int(part, 0)
        if value != 0xFF and not 1 <= value <= 6:
            raise argparse.ArgumentTypeError(f"slot {part!r} must be 1..6 or ff")
        if value not in result:
            result.append(value)
    if not result:
        raise argparse.ArgumentTypeError("provide at least one slot")
    return result


def hx(data: bytes | bytearray | list[int] | None) -> str:
    if not data:
        return "<empty>"
    return " ".join(f"{int(value):02X}" for value in data)


def path_text(value: Any) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="backslashreplace")
    return str(value)


def fmt_num(value: Any, width: int = 4) -> str:
    try:
        return f"0x{int(value):0{width}X}"
    except (TypeError, ValueError):
        return "n/a"


def build_frame(report_id: int, report_len: int, device_index: int,
                feature_index: int, function_index: int, software_id: int,
                params: bytes = b"") -> bytes:
    """Build HID++ 2.0: [report, device, feature, fn<<4|swid, params...]."""
    parameter_capacity = report_len - 4
    if len(params) > parameter_capacity:
        raise ValueError(f"{len(params)} params exceed report capacity {parameter_capacity}")
    function_sw = ((function_index & 0x0F) << 4) | (software_id & 0x0F)
    return bytes((report_id, device_index, feature_index, function_sw)) + params + bytes(parameter_capacity - len(params))


def describe_frame(data: bytes | list[int]) -> str:
    raw = bytes(data)
    if len(raw) < 4:
        return f"short/unknown frame len={len(raw)} raw=[{hx(raw)}]"
    if raw[2] == 0xFF and len(raw) >= 6:
        # HID++ 2.0 error reply: [report, device, FF, failed feature, fn|swid, error]
        return (
            f"HID++ ERROR dev={raw[1]:02X} failedFeature={raw[3]:02X} "
            f"fn={raw[4] >> 4:X} swid={raw[4] & 0x0F:X} error={raw[5]:02X}"
        )
    return (
        f"report={raw[0]:02X} dev={raw[1]:02X} feature={raw[2]:02X} "
        f"fn={raw[3] >> 4:X} swid={raw[3] & 0x0F:X} "
        f"params=[{hx(raw[4:])}]"
    )


def response_kind(raw_data: bytes | list[int], *, device_index: int,
                  feature_index: int, function_index: int,
                  software_id: int) -> str | None:
    """Return MATCH or ERROR if this packet answers the current request."""
    raw = bytes(raw_data)
    if len(raw) < 4 or raw[1] != device_index:
        return None
    expected_function_sw = ((function_index & 0x0F) << 4) | (software_id & 0x0F)
    if raw[2] == feature_index and raw[3] == expected_function_sw:
        return "MATCH"
    if (len(raw) >= 6 and raw[2] == 0xFF and raw[3] == feature_index
            and raw[4] == expected_function_sw):
        return "ERROR"
    return None


def send_request(device: Any, log: Log, *, label: str, report_id: int,
                 report_len: int, device_index: int, feature_index: int,
                 function_index: int, software_id: int, params: bytes,
                 timeout_ms: int) -> bytes | None:
    packet = build_frame(report_id, report_len, device_index, feature_index,
                         function_index, software_id, params)
    log.write(
        f"TX {label}: len={len(packet)} path-report=0x{report_id:02X} "
        f"dev=0x{device_index:02X} feature=0x{feature_index:02X} "
        f"fn={function_index} swid={software_id} bytes=[{hx(packet)}]"
    )
    try:
        written = device.write(list(packet))
        log.write(f"TX result: write() returned {written} (requested {len(packet)} bytes)")
        if written is not None and written < 0:
            log.write("TX WARNING: negative write result")
    except Exception as exc:  # hidapi exception messages differ by version/Windows build
        log.write(f"TX FAILED: {type(exc).__name__}: {exc!r}")
        return None

    deadline = time.monotonic() + timeout_ms / 1000.0
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        try:
            rx_list = device.read(64, max(1, int(remaining * 1000)))
        except Exception as exc:
            log.write(f"RX FAILED: {type(exc).__name__}: {exc!r}")
            break
        if not rx_list:
            break
        raw = bytes(rx_list)
        kind = response_kind(raw, device_index=device_index,
                             feature_index=feature_index,
                             function_index=function_index,
                             software_id=software_id)
        if kind:
            log.write(f"RX_{kind} {label}: {describe_frame(raw)} bytes=[{hx(raw)}]")
            return raw
        log.write(f"RX_UNMATCHED {label}: {describe_frame(raw)} bytes=[{hx(raw)}]")
    log.write(f"TIMEOUT {label}: no matching response within {timeout_ms} ms")
    return None


def get_feature(device: Any, log: Log, *, report_id: int, report_len: int,
                device_index: int, feature_id: int, software_id: int,
                timeout_ms: int) -> tuple[int | None, bytes | None]:
    name = FEATURE_NAMES.get(feature_id, "unknown feature")
    response = send_request(
        device, log,
        label=f"GetFeature(0x{feature_id:04X} {name}) slot=0x{device_index:02X}",
        report_id=report_id, report_len=report_len,
        device_index=device_index, feature_index=ROOT_FEATURE_INDEX,
        function_index=0, software_id=software_id,
        params=bytes((feature_id >> 8, feature_id & 0xFF, 0x00)),
        timeout_ms=timeout_ms,
    )
    if not response:
        return None, None
    if response[2] == 0xFF:
        return None, response
    if len(response) < 7:
        log.write(f"WARNING: GetFeature response shorter than expected: {len(response)} bytes")
        return None, response
    index, feature_type, version = response[4], response[5], response[6]
    status = "UNSUPPORTED" if index == 0 else "SUPPORTED"
    log.write(
        f"FEATURE RESULT 0x{feature_id:04X}: {status}, index=0x{index:02X}, "
        f"type=0x{feature_type:02X}, version=0x{version:02X}"
    )
    return index, response


def probe_battery_feature(device: Any, log: Log, *, report_id: int,
                          report_len: int, device_index: int,
                          feature_id: int, feature_index: int,
                          software_id: int, timeout_ms: int) -> None:
    if feature_id == 0x1000:
        function_index = 0  # legacy Battery Status: GetBatteryLevelStatus
        label = "BatteryStatus.GetBatteryLevelStatus"
    elif feature_id == 0x1004:
        function_index = 1  # Unified Battery: GetStatus
        label = "UnifiedBattery.GetStatus"
    else:
        return
    response = send_request(
        device, log,
        label=f"{label} slot=0x{device_index:02X} featureIndex=0x{feature_index:02X}",
        report_id=report_id, report_len=report_len,
        device_index=device_index, feature_index=feature_index,
        function_index=function_index, software_id=software_id,
        params=b"", timeout_ms=timeout_ms,
    )
    if response and response[2] != 0xFF:
        params = response[4:]
        log.write(f"BATTERY RAW PARAMS: [{hx(params)}]")
        if feature_id == 0x1000 and len(params) >= 3:
            log.write(
                f"BATTERY 0x1000 decode (protocol fields): level={params[0]}%, "
                f"next-level={params[1]}%, status={params[2]}"
            )
        elif feature_id == 0x1004 and len(params) >= 4:
            log.write(
                f"BATTERY 0x1004 decode (protocol fields): charge={params[0]}%, "
                f"level-flags=0x{params[1]:02X}, charging-status={params[2]}, "
                f"external-power={params[3]}"
            )


def main() -> int:
    parser = argparse.ArgumentParser(description="Read-only Logitech HID++ receiver diagnostic")
    parser.add_argument("--vid", type=parse_int, default=DEFAULT_VID, help="USB vendor ID (default 0x046D)")
    parser.add_argument("--pid", type=parse_int, default=DEFAULT_PID, help="USB product ID (default 0xC547)")
    parser.add_argument("--slots", type=parse_slots, default=parse_slots("ff,1,2,3,4,5,6"),
                        help="comma-separated receiver slots, 1..6, plus ff for receiver/direct (default: ff,1,2,3,4,5,6)")
    parser.add_argument("--timeout-ms", type=int, default=DEFAULT_TIMEOUT_MS,
                        help=f"wait per request (default {DEFAULT_TIMEOUT_MS} ms)")
    parser.add_argument("--swid", type=int, default=1,
                        help="starting software ID nibble 1..15 (default 1; requests cycle to avoid stale matches)")
    parser.add_argument("--log", type=Path, default=None,
                        help="log path (default: hidpp_debug_YYYYMMDD_HHMMSS.txt in current directory)")
    args = parser.parse_args()

    if not 1 <= args.swid <= 15:
        parser.error("--swid must be from 1 through 15; zero is reserved for unsolicited events")
    if args.timeout_ms < 50:
        parser.error("--timeout-ms must be at least 50")

    log_path = args.log or Path.cwd() / f"hidpp_debug_{_dt.datetime.now().strftime('%Y%m%d_%H%M%S')}.txt"
    log = Log(log_path)
    try:
        log.write("Logitech HID++ 2.0 read-only diagnostic")
        log.write(f"Python: {sys.version.split()[0]} | OS: {os.name} ({sys.platform})")
        log.write(f"Target VID:PID = {args.vid:04X}:{args.pid:04X}")
        log.write("Framing: [reportID, deviceIndex, featureIndex, (function<<4)|softwareID, parameters]")
        log.write("Short report: 7 bytes / 3 parameter bytes; long report: 20 bytes / 16 parameter bytes")
        log.write("No settings, pairing, or firmware commands are sent.")

        try:
            infos = hid.enumerate(args.vid, args.pid)
        except Exception as exc:
            log.write(f"ENUMERATION FAILED: {type(exc).__name__}: {exc!r}")
            return 2

        if not infos:
            log.write("NO MATCHING HID INTERFACES FOUND.")
            log.write("Check VID/PID and confirm the receiver is connected. Try --pid 0xC547 for a Logi Bolt receiver.")
            return 1

        log.write(f"Enumerated {len(infos)} HID interface(s):")
        for i, info in enumerate(infos):
            log.write(
                f"  [{i}] path={path_text(info.get('path'))!r} "
                f"VID:PID={fmt_num(info.get('vendor_id'))}:{fmt_num(info.get('product_id'))} "
                f"usagePage={fmt_num(info.get('usage_page'))} usage={fmt_num(info.get('usage'))} "
                f"interface={info.get('interface_number', 'n/a')} "
                f"manufacturer={info.get('manufacturer_string')!r} "
                f"product={info.get('product_string')!r} "
                f"serial={info.get('serial_number')!r} "
                f"release={fmt_num(info.get('release_number'))}"
            )

        candidates: list[tuple[dict[str, Any], int, int, int]] = []
        for info in infos:
            usage_page = info.get("usage_page")
            usage = info.get("usage")
            if usage_page != HIDPP_USAGE_PAGE or usage not in (SHORT_USAGE, LONG_USAGE):
                continue
            if usage == SHORT_USAGE:
                candidates.append((info, SHORT_REPORT_ID, SHORT_REPORT_LEN, usage))
            else:
                candidates.append((info, LONG_REPORT_ID, LONG_REPORT_LEN, usage))

        if not candidates:
            log.write("NO HID++ vendor collection (usage page 0xFF00, usage 0x0001/0x0002) found.")
            log.write("All matching VID/PID interfaces were listed above; do not guess report lengths.")
            return 1

        swid_counter = args.swid
        any_open = False
        any_match = False
        for info, report_id, report_len, usage in candidates:
            log.write("=" * 88)
            log.write(
                f"OPEN candidate usagePage=0x{HIDPP_USAGE_PAGE:04X} usage=0x{usage:04X}; "
                f"expected output report ID=0x{report_id:02X}, length={report_len}"
            )
            dev = hid.device()
            try:
                dev.open_path(info["path"])
                any_open = True
                try:
                    dev.set_nonblocking(0)
                except Exception as exc:
                    log.write(f"Note: set_nonblocking(0) unavailable: {exc!r}")
                log.write("OPEN OK")
            except Exception as exc:
                log.write(f"OPEN FAILED: {type(exc).__name__}: {exc!r}")
                try:
                    dev.close()
                except Exception:
                    pass
                continue

            try:
                responsive_slots: list[int] = []
                for slot in args.slots:
                    swid = swid_counter
                    swid_counter = (swid_counter % 15) + 1
                    feature_index, _ = get_feature(
                        dev, log, report_id=report_id, report_len=report_len,
                        device_index=slot, feature_id=0x0003,
                        software_id=swid, timeout_ms=args.timeout_ms,
                    )
                    if feature_index is not None:
                        # GetFeature(0x0003) is a read-only sanity check for correct framing/routing.
                        any_match = True
                        responsive_slots.append(slot)
                    elif feature_index == 0:
                        # A returned zero is a valid match but indicates the feature is absent.
                        any_match = True
                        responsive_slots.append(slot)

                # Only continue with additional feature/battery requests on slots that answered.
                for slot in responsive_slots:
                    found: dict[int, int] = {}
                    for feature_id in (0x1000, 0x1001, 0x1004):
                        swid = swid_counter
                        swid_counter = (swid_counter % 15) + 1
                        index, _ = get_feature(
                            dev, log, report_id=report_id, report_len=report_len,
                            device_index=slot, feature_id=feature_id,
                            software_id=swid, timeout_ms=args.timeout_ms,
                        )
                        if index is not None and index != 0:
                            found[feature_id] = index
                    for feature_id in (0x1000, 0x1004):
                        if feature_id in found:
                            swid = swid_counter
                            swid_counter = (swid_counter % 15) + 1
                            probe_battery_feature(
                                dev, log, report_id=report_id, report_len=report_len,
                                device_index=slot, feature_id=feature_id,
                                feature_index=found[feature_id], software_id=swid,
                                timeout_ms=args.timeout_ms,
                            )
            finally:
                try:
                    dev.close()
                    log.write("CLOSED HID handle")
                except Exception as exc:
                    log.write(f"HID close note: {exc!r}")

        log.write("=" * 88)
        if not any_open:
            log.write("SUMMARY: No candidate HID++ interface could be opened.")
            log.write("This is an interface/driver/access problem, before HID++ response parsing can be judged.")
        elif not any_match:
            log.write("SUMMARY: No request received a matching HID++ response on the tested interfaces/slots.")
            log.write("Use the TX/RX lines to distinguish write failure, no input, and unrelated swid=0 events.")
            log.write("If usage 0x0001 fails writes but usage 0x0002 accepts them, that points to report/collection transport differences.")
        else:
            log.write("SUMMARY: At least one request received a matching HID++ response.")
            log.write("Check FEATURE RESULT and BATTERY lines to see whether routing, function matching, and battery reads work.")
        log.write(f"Full log saved to: {log_path.resolve()}")
        return 0
    finally:
        log.close()


if __name__ == "__main__":
    raise SystemExit(main())
