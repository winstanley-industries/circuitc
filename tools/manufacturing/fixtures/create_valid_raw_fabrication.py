#!/usr/bin/env python3
"""Create deterministic, structurally valid KiCad 10.0.5 raw test outputs."""

from __future__ import annotations

import hashlib
import json
import pathlib
import sys

LAYERS = [
    ("F.Cu", "Copper,L1,Top", "Copper,L1,Top", "Positive", "F_Cu"),
    ("F.Mask", "Soldermask,Top", "SolderMask,Top", "Negative", "F_Mask"),
    ("B.Cu", "Copper,L2,Bot", "Copper,L2,Bot", "Positive", "B_Cu"),
    ("B.Mask", "Soldermask,Bot", "SolderMask,Bot", "Negative", "B_Mask"),
    ("F.SilkS", "Legend,Top", "Legend,Top", "Positive", "F_Silkscreen"),
    ("B.SilkS", "Legend,Bot", "Legend,Bot", "Positive", "B_Silkscreen"),
    ("F.Paste", "Paste,Top", "SolderPaste,Top", "Positive", "F_Paste"),
    ("B.Paste", "Paste,Bot", "SolderPaste,Bot", "Positive", "B_Paste"),
    ("Edge.Cuts", "Profile,NP", "Profile", "Positive", "Edge_Cuts"),
]
CREATION_DATE = "2026-08-04T08:00:01-07:00"


def sha256(contents: bytes) -> str:
    return hashlib.sha256(contents).hexdigest()


def gerber(design_name: str, layer: tuple[str, str, str, str, str]) -> bytes:
    layer_name, file_function, _, polarity, _ = layer
    file_polarity = "" if layer_name == "Edge.Cuts" else f"%TF.FilePolarity,{polarity}*%\n"
    return (
        "%TF.GenerationSoftware,KiCad,Pcbnew,10.0.5*%\n"
        f"%TF.CreationDate,{CREATION_DATE}*%\n"
        f"%TF.ProjectId,{design_name},00000000-0000-0000-0000-000000000000,rev?*%\n"
        "%TF.SameCoordinates,Original*%\n"
        f"%TF.FileFunction,{file_function}*%\n"
        f"{file_polarity}"
        "%FSLAX46Y46*%\n"
        "G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\n"
        "G04 Created by KiCad (PCBNEW 10.0.5) date 2026-08-04 08:00:01*\n"
        "%MOMM*%\n"
        "%LPD*%\n"
        "G01*\n"
        "M02*\n"
    ).encode()


def gerber_job(design_name: str) -> bytes:
    value = {
        "Header": {
            "GenerationSoftware": {
                "Vendor": "KiCad",
                "Application": "Pcbnew",
                "Version": "10.0.5",
            },
            "CreationDate": CREATION_DATE,
        },
        "GeneralSpecs": {"ProjectId": {"Name": design_name}},
        "FilesAttributes": [
            {
                "Path": f"{design_name}-{filename_layer}.gbr",
                "FileFunction": job_function,
                "FilePolarity": polarity,
            }
            for _, _, job_function, polarity, filename_layer in LAYERS
        ],
    }
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def drill(plated: bool) -> bytes:
    function = "Plated,1,2,PTH" if plated else "NonPlated,1,2,NPTH"
    return (
        "M48\n"
        "; DRILL file KiCad 10.0.5 date 2026-08-04T08:00:01\n"
        f"; FORMAT={{-:-/ absolute / metric / decimal}}\n"
        f"; #@! TF.CreationDate,{CREATION_DATE}\n"
        "; #@! TF.GenerationSoftware,Kicad,Pcbnew,10.0.5\n"
        f"; #@! TF.FileFunction,{function}\n"
        "FMAT,2\n"
        "METRIC\n"
        "%\n"
        "G90\n"
        "G05\n"
        "M30\n"
    ).encode()


def main() -> None:
    if len(sys.argv) != 6:
        raise SystemExit(
            "usage: create_valid_raw_fabrication.py ROOT DESIGN REQUEST BOARD EXECUTABLE"
        )
    root = pathlib.Path(sys.argv[1])
    design_name = sys.argv[2]
    request = pathlib.Path(sys.argv[3]).read_bytes()
    board = pathlib.Path(sys.argv[4]).read_bytes()
    executable = pathlib.Path(sys.argv[5]).read_bytes()
    outputs: dict[str, bytes] = {}
    for layer in LAYERS:
        filename_layer = layer[4]
        outputs[f"gerber/{design_name}-{filename_layer}.gbr"] = gerber(design_name, layer)
    outputs[f"gerber/{design_name}-job.gbrjob"] = gerber_job(design_name)
    outputs[f"drill/{design_name}-NPTH.drl"] = drill(False)
    outputs[f"drill/{design_name}-PTH.drl"] = drill(True)
    outputs[f"position/{design_name}-all-pos.csv"] = (
        "Ref,Val,Package,PosX,PosY,Rot,Side\n"
        '"R1","10kΩ","R_0603_1608Metric",15.000000,-10.000000,0.000000,top\n'
        '"R2","10kΩ","R_0603_1608Metric",25.000000,-10.000000,0.000000,top\n'
    ).encode()
    for relative_path, contents in outputs.items():
        destination = root / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(contents)
    receipt = {
        "schema_name": "circuitc.kicad_fabrication_receipt",
        "schema_version": 1,
        "request_sha256": sha256(request),
        "board_sha256": sha256(board),
        "executable_sha256": sha256(executable),
        "outputs": [
            {"path": path, "sha256": sha256(contents)} for path, contents in sorted(outputs.items())
        ],
    }
    receipt_path = root / "receipt/host.json"
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(
        json.dumps(receipt, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
