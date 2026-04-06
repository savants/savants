"""BYOK (Bring Your Own Key) Envelope Encryption.

Implements a two-layer encryption scheme:
  1. Data Encryption Key (DEK): encrypts the actual graph data
  2. Master Key (MK): wraps the DEK — provided by the user's KMS

The user maintains total control. Revoking KMS access = instant lockout
(cryptographic kill-switch).
"""

from __future__ import annotations

import logging
import os
import secrets
from dataclasses import dataclass

from cryptography.fernet import Fernet
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
import base64

logger = logging.getLogger(__name__)


@dataclass
class EncryptedPayload:
    """An encrypted graph snapshot with its wrapped DEK."""

    encrypted_data: bytes
    encrypted_dek: bytes
    salt: bytes
    nonce: bytes = b""


def generate_dek() -> bytes:
    """Generate a new Data Encryption Key."""
    return Fernet.generate_key()


def derive_key_from_passphrase(passphrase: str, salt: bytes | None = None) -> tuple[bytes, bytes]:
    """Derive an encryption key from a user passphrase (local BYOK variant)."""
    if salt is None:
        salt = os.urandom(16)

    kdf = PBKDF2HMAC(
        algorithm=hashes.SHA256(),
        length=32,
        salt=salt,
        iterations=600_000,
    )
    key = base64.urlsafe_b64encode(kdf.derive(passphrase.encode()))
    return key, salt


def encrypt_data(data: bytes, dek: bytes) -> bytes:
    """Encrypt data using the DEK (Fernet symmetric encryption)."""
    f = Fernet(dek)
    return f.encrypt(data)


def decrypt_data(encrypted_data: bytes, dek: bytes) -> bytes:
    """Decrypt data using the DEK."""
    f = Fernet(dek)
    return f.decrypt(encrypted_data)


def wrap_dek_with_passphrase(dek: bytes, passphrase: str) -> tuple[bytes, bytes]:
    """Wrap (encrypt) the DEK using a user-provided passphrase.

    For local-first users who don't have a cloud KMS.
    """
    wrapping_key, salt = derive_key_from_passphrase(passphrase)
    f = Fernet(wrapping_key)
    encrypted_dek = f.encrypt(dek)
    return encrypted_dek, salt


def unwrap_dek_with_passphrase(encrypted_dek: bytes, passphrase: str, salt: bytes) -> bytes:
    """Unwrap (decrypt) the DEK using the user's passphrase."""
    wrapping_key, _ = derive_key_from_passphrase(passphrase, salt)
    f = Fernet(wrapping_key)
    return f.decrypt(encrypted_dek)


def encrypt_snapshot(
    graph_data: bytes,
    passphrase: str,
) -> EncryptedPayload:
    """Encrypt a graph snapshot using envelope encryption.

    1. Generate a random DEK
    2. Encrypt the graph data with the DEK
    3. Wrap the DEK with the user's passphrase
    4. Return the encrypted bundle
    """
    dek = generate_dek()
    encrypted_data = encrypt_data(graph_data, dek)
    encrypted_dek, salt = wrap_dek_with_passphrase(dek, passphrase)

    return EncryptedPayload(
        encrypted_data=encrypted_data,
        encrypted_dek=encrypted_dek,
        salt=salt,
    )


def decrypt_snapshot(
    payload: EncryptedPayload,
    passphrase: str,
) -> bytes:
    """Decrypt a graph snapshot using the user's passphrase.

    1. Unwrap the DEK using the passphrase
    2. Decrypt the graph data with the DEK
    3. Return the plaintext graph data
    """
    dek = unwrap_dek_with_passphrase(payload.encrypted_dek, passphrase, payload.salt)
    return decrypt_data(payload.encrypted_data, dek)
