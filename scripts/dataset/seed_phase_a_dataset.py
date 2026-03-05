#!/usr/bin/env python3
"""Seed Phase A dataset samples and manifests with deterministic assets."""

from __future__ import annotations

import hashlib
import json
from datetime import date
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
TODAY = date.today().isoformat()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def save_rgba(arr: np.ndarray, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(arr.astype(np.uint8), mode="RGBA").save(path, optimize=True)


def make_quality_samples() -> list[dict]:
    out = DATASET / "quality"
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(42)

    # Smooth gradient photo-like image.
    w1, h1 = 960, 540
    x = np.linspace(0, 1, w1, dtype=np.float32)[None, :]
    y = np.linspace(0, 1, h1, dtype=np.float32)[:, None]
    r = (255 * (0.15 + 0.85 * x)).repeat(h1, axis=0)
    g = (255 * (0.10 + 0.90 * y)).repeat(w1, axis=1)
    b = 255 * (0.2 + 0.8 * (0.6 * x + 0.4 * y))
    noise = rng.normal(0, 6, size=(h1, w1)).astype(np.float32)
    img1 = np.stack([r + noise, g + noise, b + noise, np.full((h1, w1), 255)], axis=2)
    img1 = np.clip(img1, 0, 255)
    f1 = out / "q_gradient_photo_like.png"
    save_rgba(img1, f1)

    # UI/icon-like alpha edges.
    w2, h2 = 512, 512
    base = Image.new("RGBA", (w2, h2), (0, 0, 0, 0))
    draw = ImageDraw.Draw(base, "RGBA")
    draw.rounded_rectangle((40, 40, 472, 472), radius=90, fill=(18, 121, 201, 220))
    draw.rounded_rectangle((80, 80, 432, 432), radius=60, fill=(248, 250, 252, 245))
    draw.ellipse((140, 140, 372, 372), fill=(234, 88, 12, 230))
    draw.polygon([(256, 170), (320, 300), (192, 300)], fill=(255, 255, 255, 240))
    f2 = out / "q_ui_alpha_icon.png"
    base.save(f2, optimize=True)

    # Low-color block/gradient mix.
    w3, h3 = 640, 360
    palette = np.array(
        [
            [0, 0, 0],
            [31, 41, 55],
            [55, 65, 81],
            [107, 114, 128],
            [156, 163, 175],
            [17, 24, 39],
            [3, 105, 161],
            [14, 116, 144],
            [13, 148, 136],
            [21, 128, 61],
            [74, 222, 128],
            [245, 158, 11],
            [249, 115, 22],
            [239, 68, 68],
            [244, 63, 94],
            [217, 70, 239],
            [126, 34, 206],
            [79, 70, 229],
            [59, 130, 246],
            [6, 182, 212],
            [20, 184, 166],
            [132, 204, 22],
            [163, 230, 53],
            [250, 204, 21],
            [251, 146, 60],
            [248, 113, 113],
            [251, 113, 133],
            [232, 121, 249],
            [167, 139, 250],
            [96, 165, 250],
            [45, 212, 191],
            [255, 255, 255],
        ],
        dtype=np.uint8,
    )
    idx = ((np.arange(h3)[:, None] // 24 + np.arange(w3)[None, :] // 24) % len(palette)).astype(np.int32)
    img3 = np.zeros((h3, w3, 4), dtype=np.uint8)
    img3[:, :, :3] = palette[idx]
    img3[:, :, 3] = 255
    f3 = out / "q_lowcolor_blocks.png"
    save_rgba(img3, f3)

    return [
        manifest_item(
            "quality-001-gradient-photo",
            f1,
            "960x540",
            ["quality", "photo-like", "gradient", "noise"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed baseline quality dataset",
            expected_success=True,
        ),
        manifest_item(
            "quality-002-ui-alpha-icon",
            f2,
            "512x512",
            ["quality", "ui", "icon", "alpha-edge"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed baseline alpha/UI quality dataset",
            expected_success=True,
        ),
        manifest_item(
            "quality-003-lowcolor-blocks",
            f3,
            "640x360",
            ["quality", "low-color", "blocks"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed baseline low-color quality dataset",
            expected_success=True,
        ),
    ]


def make_perf_samples() -> list[dict]:
    out = DATASET / "perf"
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(7)

    # Large image for throughput testing.
    w1, h1 = 2400, 1600
    x = np.linspace(0, 1, w1, dtype=np.float32)[None, :]
    y = np.linspace(0, 1, h1, dtype=np.float32)[:, None]
    base = np.zeros((h1, w1, 4), dtype=np.float32)
    base[:, :, 0] = 255 * (0.5 * x + 0.5 * y)
    base[:, :, 1] = 255 * (0.2 + 0.8 * x)
    base[:, :, 2] = 255 * (0.7 * (1 - y) + 0.2 * x)
    base[:, :, 3] = 255
    noise = rng.normal(0, 12, size=(h1, w1, 1)).astype(np.float32)
    img1 = np.clip(base[:, :, :3] + noise, 0, 255)
    img1 = np.concatenate([img1, np.full((h1, w1, 1), 255, dtype=np.float32)], axis=2)
    f1 = out / "p_large_gradient_noise.png"
    save_rgba(img1, f1)

    # Medium-large alpha pattern.
    w2, h2 = 2048, 2048
    yy, xx = np.indices((h2, w2))
    r = ((xx // 32 + yy // 64) % 256).astype(np.uint8)
    g = ((xx // 48 + yy // 48) % 256).astype(np.uint8)
    b = ((xx // 80 + yy // 28) % 256).astype(np.uint8)
    a = ((180 + 75 * np.sin(xx / 120.0) * np.cos(yy / 100.0))).clip(0, 255).astype(np.uint8)
    img2 = np.stack([r, g, b, a], axis=2)
    f2 = out / "p_large_alpha_pattern.png"
    save_rgba(img2, f2)

    return [
        manifest_item(
            "perf-001-large-gradient-noise",
            f1,
            "2400x1600",
            ["perf", "large", "noise", "gradient"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed baseline perf dataset",
            expected_success=True,
        ),
        manifest_item(
            "perf-002-large-alpha-pattern",
            f2,
            "2048x2048",
            ["perf", "large", "alpha", "pattern"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed baseline perf alpha dataset",
            expected_success=True,
        ),
    ]


def make_robustness_samples() -> list[dict]:
    out = DATASET / "robustness"
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(99)

    src = DATASET / "quality" / "q_gradient_photo_like.png"
    src_bytes = src.read_bytes()
    f1 = out / "r_truncated_png.png"
    f1.write_bytes(src_bytes[: max(64, len(src_bytes) // 3)])

    f2 = out / "r_garbage_png.png"
    f2.write_bytes(rng.integers(0, 256, size=1024, dtype=np.uint8).tobytes())

    return [
        manifest_item(
            "robust-001-truncated-png",
            f1,
            "n/a",
            ["robustness", "corrupted", "truncated"],
            "generated-from-quality-001",
            "seed corrupted input robustness set",
            expected_success=False,
        ),
        manifest_item(
            "robust-002-garbage-png",
            f2,
            "n/a",
            ["robustness", "corrupted", "invalid-bytes"],
            "generated-by-scripts/dataset/seed_phase_a_dataset.py",
            "seed invalid input robustness set",
            expected_success=False,
        ),
    ]


def manifest_item(
    item_id: str,
    path: Path,
    resolution: str,
    scene_tags: list[str],
    source: str,
    added_reason: str,
    *,
    expected_success: bool,
) -> dict:
    return {
        "id": item_id,
        "filename": path.name,
        "resolution": resolution,
        "scene_tags": scene_tags,
        "source": source,
        "sha256": sha256_file(path),
        "added_at": TODAY,
        "added_reason": added_reason,
        "expected_success": expected_success,
    }


def write_manifest(split: str, items: list[dict]) -> None:
    manifest = DATASET / split / "manifest.json"
    manifest.write_text(json.dumps(items, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    quality_items = make_quality_samples()
    perf_items = make_perf_samples()
    robust_items = make_robustness_samples()
    write_manifest("quality", quality_items)
    write_manifest("perf", perf_items)
    write_manifest("robustness", robust_items)
    print("Seeded quality/perf/robustness datasets and manifests.")


if __name__ == "__main__":
    main()
