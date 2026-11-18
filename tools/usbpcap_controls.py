"""Lists HID class control transfers (SET_REPORT / GET_REPORT) from a USBPcap
capture, plus interrupt-IN packets that start with a given byte prefix.

Everything else (mouse movement, key presses of other devices) is skipped,
so the output is safe to share.

    python tools/usbpcap_controls.py hub1.pcap [--prefix 03] [--device N]
"""

import argparse
import struct
import sys

TRANSFER_INTERRUPT = 1
TRANSFER_CONTROL = 2


def packets(path):
    with open(path, "rb") as f:
        header = f.read(24)
        magic, _, _, _, _, _, linktype = struct.unpack("<IHHiIII", header)
        if magic != 0xA1B2C3D4 or linktype != 249:
            sys.exit(f"not a USBPcap capture (magic {magic:#x}, linktype {linktype})")
        while True:
            rec = f.read(16)
            if len(rec) < 16:
                return
            sec, usec, caplen, _ = struct.unpack("<IIII", rec)
            data = f.read(caplen)
            if len(data) < 27:
                continue
            (hlen, _irp, _status, _func, info, _bus, device, endpoint, transfer, _dlen) = (
                struct.unpack_from("<HQIHBHHBBI", data, 0)
            )
            stage = data[27] if transfer == TRANSFER_CONTROL and hlen >= 28 else None
            yield sec + usec / 1e6, device, endpoint, transfer, info & 1, stage, data[hlen:]


def hexs(b, limit=40):
    text = " ".join(f"{x:02x}" for x in b[:limit])
    return text + (f" ... ({len(b)} bytes)" if len(b) > limit else "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcap")
    ap.add_argument("--prefix", default="03", help="hex prefix of interrupt-IN packets to show")
    ap.add_argument("--device", type=int, help="only this USB device address")
    args = ap.parse_args()
    prefix = bytes.fromhex(args.prefix)

    start = None
    pending = {}  # device -> description of the last HID setup packet
    for t, device, endpoint, transfer, from_device, stage, payload in packets(args.pcap):
        if start is None:
            start = t
        if args.device is not None and device != args.device:
            continue
        rel = t - start

        if transfer == TRANSFER_INTERRUPT and endpoint & 0x80 and payload.startswith(prefix):
            print(f"{rel:8.3f} dev {device:>2} ep {endpoint:#04x} IN    {hexs(payload)}")
            continue
        if transfer != TRANSFER_CONTROL:
            continue

        if stage == 0 and len(payload) >= 8:
            req_type, request, value, index, length = struct.unpack_from("<BBHHH", payload, 0)
            if req_type & 0x60 != 0x20:  # HID class requests only
                pending.pop(device, None)
                continue
            name = {0x09: "SET_REPORT", 0x01: "GET_REPORT", 0x0A: "SET_IDLE"}.get(
                request, f"request {request:#04x}"
            )
            kind = {1: "input", 2: "output", 3: "feature"}.get(value >> 8, "?")
            desc = f"{name} {kind} id {value & 0xff:#04x} iface {index} len {length}"
            pending[device] = desc
            extra = payload[8:]
            print(f"{rel:8.3f} dev {device:>2} {desc}" + (f"\n{'':20}data {hexs(extra, 64)}" if extra else ""))
        elif device in pending and payload:
            direction = "reply" if from_device else "data "
            print(f"{rel:8.3f} dev {device:>2}   {direction} {hexs(payload, 64)}")


if __name__ == "__main__":
    main()
