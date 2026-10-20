"""Local CLI SDK. Values are explicit; errors and repr never contain credentials."""
import base64
import json
import subprocess


class SecretError(Exception):
    pass


class Secret:
    __slots__ = ("__value",)

    def __init__(self, value: bytes):
        self.__value = bytearray(value)

    def expose(self) -> bytes:
        return bytes(self.__value)

    def close(self):
        self.__value[:] = b"\0" * len(self.__value)

    def __repr__(self):
        return "Secret([redacted])"

    __str__ = __repr__

    def __del__(self):
        self.close()


_ERRORS = frozenset(("not_initialized", "not_found", "store_unavailable", "invalid_input",
                    "invalid_record", "untrusted_writer", "not_writer", "not_granted",
                    "stale_revision", "conflict", "pending_history", "storage_failure",
                    "crypto_failure", "too_large"))


class Client:
    def __init__(self, binary="hey-boss"):
        self.binary = binary

    def _call(self, request):
        try:
            result = subprocess.run([self.binary, "secrets", "rpc"],
                                    input=json.dumps(request).encode(), capture_output=True,
                                    timeout=45, check=False)
            if result.returncode or len(result.stdout) > 2 * 1024 * 1024:
                raise ValueError()
            response = json.loads(result.stdout)
        except Exception:
            raise SecretError("store_unavailable") from None
        if response.get("ok") is not True:
            code = response.get("error")
            raise SecretError(code if code in _ERRORS else "store_unavailable")
        return response

    def get(self, name):
        response = self._call({"op": "get", "name": name})
        try:
            return response["record"], Secret(base64.b64decode(response["data"], validate=True))
        except Exception:
            raise SecretError("invalid_record") from None

    def put(self, name, value: bytes, kind="value", expected=None):
        if len(value) > 1024 * 1024:
            raise SecretError("too_large")
        return self._call({"op": "put", "name": name, "kind": kind, "expected": expected,
                           "data": base64.b64encode(value).decode()})["record"]
