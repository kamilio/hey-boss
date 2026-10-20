import os
import unittest
from hey_boss_secrets import Client, Secret, SecretError

class SDKTests(unittest.TestCase):
    def test_redaction(self):
        secret = Secret(b'synthetic-only')
        self.assertEqual(repr(secret), 'Secret([redacted])')
        self.assertEqual(secret.expose(), b'synthetic-only')
        secret.close()
        self.assertEqual(secret.expose(), bytes(14))

    def test_sanitized_process_failure(self):
        with self.assertRaisesRegex(SecretError, '^store_unavailable$'):
            Client('/does/not/exist').get('synthetic')

    @unittest.skipUnless(os.environ.get('HEY_BOSS_SECRETS_TEST_BINARY'), 'native integration opt-in')
    def test_cli(self):
        client = Client(os.environ['HEY_BOSS_SECRETS_TEST_BINARY'])
        record = client.put('synthetic-python-sdk', b'\0binary\xff\n', kind='cookies')
        metadata, secret = client.get('synthetic-python-sdk')
        self.assertEqual(metadata['revision'], record['revision'])
        self.assertEqual(secret.expose(), b'\0binary\xff\n')
        with self.assertRaisesRegex(SecretError, '^stale_revision$'):
            client.put('synthetic-python-sdk', b'new', expected='stale')

if __name__ == '__main__': unittest.main()
