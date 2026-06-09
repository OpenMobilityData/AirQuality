#!/usr/bin/env python3
"""Extract the UFP surface grid from the Plotly HTML export into a compact
static JSON asset for the web app's native (Rust/canvas) 3D renderer.

The export embeds one `surface` trace with base64 binary arrays (Plotly
`bdata`): a float32 z grid (NaN = no data) over uniform float64 x/y axes in
km. We decode them, crop all-NaN border rows/columns, round values to whole
pt/cm³ (far below the model's uncertainty), and store the grid row-major with
nulls for gaps, plus the axis origin/step so the app can rebuild coordinates.

Usage: extract-ufp-surface.py <plotly-export.html> [out.json]

Raw data underlying the surface kindly provided by Scott Weichenthal
(corresponding author of the source paper); not redistributed here — only the
derived plot grid is committed.
"""

import base64
import json
import math
import struct
import sys


def decode(field):
    raw = base64.b64decode(field["bdata"])
    fmt = {"f4": "f", "f8": "d"}[field["dtype"]]
    n = len(raw) // struct.calcsize(fmt)
    return list(struct.unpack("<%d%s" % (n, fmt), raw))


def main():
    src = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "static/data/ufp-surface.json"

    with open(src) as f:
        html = f.read()

    # The last newPlot call is the figure itself (earlier hits are inside the
    # bundled plotly.js source). Its second argument is the JSON trace list.
    start = html.rfind("Plotly.newPlot(")
    data, _ = json.JSONDecoder().raw_decode(html, html.index("[", start))
    trace = data[0]
    assert trace["type"] == "surface"

    z = decode(trace["z"])
    xs = decode(trace["x"])
    ys = decode(trace["y"])
    ny, nx = (len(ys), len(xs))
    assert len(z) == nx * ny

    # Axes must be uniform for the origin+step encoding.
    dx = (xs[-1] - xs[0]) / (nx - 1)
    dy = (ys[-1] - ys[0]) / (ny - 1)
    assert all(abs(xs[i] - (xs[0] + i * dx)) < 1e-6 for i in range(nx))
    assert all(abs(ys[j] - (ys[0] + j * dy)) < 1e-6 for j in range(ny))

    rows = [z[j * nx : (j + 1) * nx] for j in range(ny)]

    def col_empty(i):
        return all(math.isnan(r[i]) for r in rows)

    j0, j1 = 0, ny
    while j0 < j1 and all(math.isnan(v) for v in rows[j0]):
        j0 += 1
    while j1 > j0 and all(math.isnan(v) for v in rows[j1 - 1]):
        j1 -= 1
    i0, i1 = 0, nx
    while i0 < i1 and col_empty(i0):
        i0 += 1
    while i1 > i0 and col_empty(i1 - 1):
        i1 -= 1

    flat = []
    for j in range(j0, j1):
        for v in rows[j][i0:i1]:
            flat.append(None if math.isnan(v) else round(v))

    valid = [v for v in flat if v is not None]
    doc = {
        "source": "Weichenthal et al. 2023, Environment International — "
        "combined LUR + deep-learning model, Montréal 2020",
        "unit": "pt/cm³",
        "nx": i1 - i0,
        "ny": j1 - j0,
        "x0": round(xs[0] + i0 * dx, 6),
        "dx": round(dx, 6),
        "y0": round(ys[0] + j0 * dy, 6),
        "dy": round(dy, 6),
        # Colour-scale clamp from the original figure (≈2nd–98th percentile).
        "cmin": round(trace["cmin"]),
        "cmax": round(trace["cmax"]),
        "zmin": min(valid),
        "zmax": max(valid),
        "z": flat,
    }
    with open(out, "w") as f:
        json.dump(doc, f, separators=(",", ":"))
    print(
        f"{out}: {doc['ny']}×{doc['nx']} grid (cropped from {ny}×{nx}), "
        f"{len(valid)} valid cells, z {doc['zmin']}–{doc['zmax']}"
    )


if __name__ == "__main__":
    main()
