#!/usr/bin/env python3
"""Compatibility entry point for the expanded exact/float numeric acceptance."""
from pathlib import Path
import runpy

if __name__ == '__main__':
    runpy.run_path(str(Path(__file__).with_name('verify-numbers.py')), run_name='__main__')
