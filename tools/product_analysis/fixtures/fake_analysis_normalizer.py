#!/usr/bin/env python3
import argparse

parser = argparse.ArgumentParser()
parser.add_argument("--probe", action="store_true", required=True)
parser.parse_args()
