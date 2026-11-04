#!/usr/bin/env python3
"""Compatibility entry point for the split equipment portals."""
import runpy
from pathlib import Path
if __name__=="__main__":runpy.run_path(str(Path(__file__).with_name("route-equipment-portals.py")),run_name="__main__")
