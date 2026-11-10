#!/usr/bin/env python3
"""Manage the minisign keypair and sign Windows update installers.

The native updater (installer.rs) verifies signatures against the public key
compiled into the app. minisign is the format it expects: a .minisig text file
with `untrusted comment:` / base64 / `trusted comment:` / global signature.

Commands:
  keygen --public <path> --secret <path>   generate a keypair (secret stays local)
  sign   --secret <path> --file <path>     write <file>.minisig
  verify --public <path> --file <path>     verify <file>.minisig

Requires py-minisign and cryptography:
  pip install py-minisign cryptography
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

try:
    import minisign
except ImportError:
    sys.exit("py-minisign is missing: pip install py-minisign cryptography")

UNTRUSTED_COMMENT = b"untrusted comment: "


def keygen(public: Path, secret: Path) -> None:
    if secret.exists():
        sys.exit(f"refusing to overwrite existing secret key: {secret}")
    pair = minisign.KeyPair.generate()
    public.parent.mkdir(parents=True, exist_ok=True)
    secret.parent.mkdir(parents=True, exist_ok=True)
    public.write_bytes(
        b"untrusted comment: minisign public key for Cao-SL/capwise_win\n"
        + pair.public_key.to_base64()
        + b"\n"
    )
    secret.write_bytes(
        b"untrusted comment: minisign SECRET KEY -- keep out of version control\n"
        + pair.secret_key.to_base64()
        + b"\n"
    )
    try:
        secret.chmod(0o600)
    except OSError:
        pass
    print(f"public key : {public}")
    print(f"secret key : {secret}  (keep out of version control)")


def sign(secret: Path, target: Path) -> None:
    key = minisign.SecretKey.from_file(secret)
    signature = key.sign_file(target)
    out = Path(str(target) + ".minisig")
    out.write_bytes(bytes(signature) + b"\n")
    print(f"wrote {out}")


def verify(public: Path, target: Path) -> None:
    key = minisign.PublicKey.from_file(public)
    sig_file = Path(str(target) + ".minisig")
    if not sig_file.read_bytes().startswith(UNTRUSTED_COMMENT):
        sys.exit(f"not a minisign signature file: {sig_file}")
    signature = minisign.Signature.from_file(sig_file)
    try:
        key.verify_file(target, signature)
    except Exception as error:
        sys.exit(f"signature verification FAILED for {target}: {error}")
    print(f"signature OK for {target}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    gen = sub.add_parser("keygen")
    gen.add_argument("--public", type=Path, required=True)
    gen.add_argument("--secret", type=Path, required=True)

    sg = sub.add_parser("sign")
    sg.add_argument("--secret", type=Path, required=True)
    sg.add_argument("--file", type=Path, required=True)

    vf = sub.add_parser("verify")
    vf.add_argument("--public", type=Path, required=True)
    vf.add_argument("--file", type=Path, required=True)

    args = parser.parse_args()
    if args.command == "keygen":
        keygen(args.public, args.secret)
    elif args.command == "sign":
        sign(args.secret, args.file)
    else:
        verify(args.public, args.file)


if __name__ == "__main__":
    main()
