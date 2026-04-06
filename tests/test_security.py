"""Tests for BYOK encryption and provenance stamping."""

from synapcode.security.encryption import (
    decrypt_snapshot,
    encrypt_snapshot,
    generate_dek,
    encrypt_data,
    decrypt_data,
)
from synapcode.security.provenance import (
    compute_content_hash,
    create_stamp,
    verify_stamp,
)


def test_encrypt_decrypt_roundtrip():
    dek = generate_dek()
    plaintext = b"Hello, SynapCode!"
    encrypted = encrypt_data(plaintext, dek)
    assert encrypted != plaintext
    decrypted = decrypt_data(encrypted, dek)
    assert decrypted == plaintext


def test_envelope_encryption_roundtrip():
    graph_data = b"GRAPH.DUMP serialized data here..." * 100
    passphrase = "my-secret-key-2026"

    payload = encrypt_snapshot(graph_data, passphrase)
    assert payload.encrypted_data != graph_data
    assert payload.encrypted_dek is not None

    decrypted = decrypt_snapshot(payload, passphrase)
    assert decrypted == graph_data


def test_wrong_passphrase_fails():
    graph_data = b"sensitive graph data"
    payload = encrypt_snapshot(graph_data, "correct-password")

    try:
        decrypt_snapshot(payload, "wrong-password")
        assert False, "Should have raised an exception"
    except Exception:
        pass  # Expected


def test_provenance_hash():
    content = "def hello(): pass"
    h = compute_content_hash(content)
    assert len(h) == 64  # SHA-256 hex digest


def test_provenance_stamp_verify():
    content = "class MyClass: pass"
    stamp = create_stamp(content, commit_sha="abc123", author="dev")
    assert verify_stamp(stamp, content)
    assert not verify_stamp(stamp, "modified content")
