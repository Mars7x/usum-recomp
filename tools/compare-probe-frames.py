#!/usr/bin/env python3
"""Compare private probe PPM captures without copying pixels into the report."""
import argparse
import json
import re
import sys
from pathlib import Path


def read_frame(path):
    data = path.read_bytes()
    # Probe emits this exact P6 header; do not strip whitespace from pixel data.
    header = re.match(rb'P6\n([0-9]+) ([0-9]+)\n255\n', data)
    if header is None:
        raise ValueError(f'{path.name}: not a probe PPM')
    width, height = map(int, header.groups())
    pixels = data[header.end():]
    if width == 0 or height == 0 or len(pixels) != width * height * 3:
        raise ValueError(f'{path.name}: invalid pixel count')
    return width, height, pixels


def compare(reference, candidate, threshold):
    rw, rh, rp = read_frame(reference)
    cw, ch, cp = read_frame(candidate)
    if (rw, rh) != (cw, ch):
        raise ValueError(f'{reference.name}: dimensions differ')
    delta = [abs(a - b) for a, b in zip(rp, cp)]
    pixels = rw * rh
    differing = sum(any(delta[i:i + 3]) for i in range(0, len(delta), 3))
    over = sum(max(delta[i:i + 3]) > threshold for i in range(0, len(delta), 3))
    return dict(frame=reference.name, pixels=pixels, rgb_mae=sum(delta) / len(delta),
                max_channel_error=max(delta), differing_pixels=differing,
                pixels_over_threshold=over, percent_over_threshold=100 * over / pixels)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path)
    parser.add_argument('candidate', type=Path)
    parser.add_argument('--pattern', default='*.ppm',
                        help="capture filename glob (use '??????-*.ppm' for periodic frames only)")
    parser.add_argument('--threshold', type=int, default=8,
                        help='count pixels with any channel error greater than this (default: 8)')
    args = parser.parse_args()
    if not 0 <= args.threshold <= 255:
        parser.error('threshold must be between 0 and 255')
    for path in (args.reference, args.candidate):
        if not path.is_dir():
            parser.error(f'not a directory: {path}')
    if '/' in args.pattern or '\\' in args.pattern:
        parser.error('pattern must match filenames within the capture directory')
    reference = {p.name: p for p in args.reference.glob(args.pattern) if p.is_file()}
    candidate = {p.name: p for p in args.candidate.glob(args.pattern) if p.is_file()}
    common = sorted(reference.keys() & candidate.keys())
    try:
        frames = [compare(reference[n], candidate[n], args.threshold) for n in common]
    except (OSError, ValueError) as error:
        parser.error(str(error))
    report = dict(threshold=args.threshold, compared=len(frames),
                  missing_candidate=sorted(reference.keys() - candidate.keys()),
                  missing_reference=sorted(candidate.keys() - reference.keys()), frames=frames)
    print(json.dumps(report, indent=2))
    # Differences are measurements, not a universal renderer acceptance limit.
    # Missing frames do fail the command, including an empty comparison.
    return int(not frames or bool(report['missing_candidate'] or report['missing_reference']))


if __name__ == '__main__':
    sys.exit(main())
