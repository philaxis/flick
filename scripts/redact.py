#!/usr/bin/env python3
import sys
from state import clean
for line in sys.stdin:
    print(clean(line.rstrip()), flush=True)
