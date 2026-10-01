import hashlib
import io
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest import mock

import update_network as network
import updater


class Response(io.BytesIO):
    def info(self):
        return {'Content-Length': str(len(self.getbuffer()))}


class NetworkTests(unittest.TestCase):
    def test_late_connection_is_closed_after_cancellation(self):
        entered, release, finished, cancel = (threading.Event() for _ in range(4))
        response = Response(b'content')
        errors = []

        def connect(*args):
            entered.set()
            release.wait(3)
            return response

        def download():
            try:
                network.urlopen('https://example.test/app.exe', 5, cancel_event=cancel)
            except Exception as error:
                errors.append(error)
            finally:
                finished.set()

        with mock.patch.object(network, '_open_request', side_effect=connect):
            worker = threading.Thread(target=download)
            worker.start()
            try:
                self.assertTrue(entered.wait(2))
                cancel.set()
                self.assertTrue(finished.wait(1))
                self.assertIsInstance(errors[0], network.UpdateCancelled)
            finally:
                release.set()
                worker.join(2)
        deadline = time.monotonic() + 2
        while not response.closed and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(response.closed)

    def test_cancelled_read_cannot_write_late_bytes(self):
        entered, release, cancel = (threading.Event() for _ in range(3))
        errors = []

        class Blocked(Response):
            def read1(self, size=-1):
                entered.set()
                release.wait(3)
                return b'late-bytes'

        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'asset.partial'
            response = Blocked(b'late-bytes')

            def download():
                try:
                    updater._download_to_path('https://example.test/app.exe', str(path), expected_size=10, cancel_event=cancel)
                except Exception as error:
                    errors.append(error)

            with mock.patch.object(updater, '_urlopen', return_value=response):
                worker = threading.Thread(target=download)
                worker.start()
                try:
                    self.assertTrue(entered.wait(2))
                    cancel.set()
                    worker.join(1)
                    self.assertFalse(worker.is_alive())
                    self.assertIsInstance(errors[0], network.UpdateCancelled)
                    self.assertEqual(path.read_bytes(), b'')
                    path.unlink()
                finally:
                    release.set()
                    worker.join(2)
                time.sleep(0.05)
                self.assertFalse(path.exists())

    def test_continuously_slow_source_is_rejected(self):
        clock = [0.0]

        class Slow(Response):
            def read1(self, size=-1):
                clock[0] += 0.5
                return super().read(1)

        with tempfile.TemporaryDirectory() as root, mock.patch.object(updater, '_urlopen', return_value=Slow(b'a' * 100)), mock.patch.object(updater.time, 'monotonic', side_effect=lambda: clock[0]), mock.patch.object(updater, 'DOWNLOAD_RATE_WINDOW', 1):
            with self.assertRaisesRegex(TimeoutError, '低速'):
                updater._download_to_path('https://example.test/app.exe', str(Path(root) / 'asset'), expected_size=100)

    def test_deadline_bounds_a_blocked_connection(self):
        release = threading.Event()
        response = Response(b'content')
        try:
            with mock.patch.object(network, '_open_request', side_effect=lambda *args: (release.wait(3), response)[1]):
                started = time.monotonic()
                with self.assertRaises(TimeoutError):
                    network.urlopen('https://example.test/app.exe', 5, deadline=started + 0.15)
                self.assertLess(time.monotonic() - started, 1)
        finally:
            release.set()

    def test_real_http_transport_and_disk_digest(self):
        content = b'MZ' + b'a' * (1024 * 1024)

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header('Content-Length', str(len(content)))
                self.end_headers()
                self.wfile.write(content)

            def log_message(self, *args):
                pass

        with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                with tempfile.TemporaryDirectory() as root:
                    path = Path(root) / 'asset.partial'
                    size, digest = updater._download_to_path(f'http://127.0.0.1:{server.server_port}/asset', str(path), expected_size=len(content))
                    self.assertEqual((size, digest), (len(content), hashlib.sha256(content).hexdigest()))
                    self.assertEqual(path.read_bytes(), content)
            finally:
                server.shutdown()
                worker.join(2)


if __name__ == '__main__':
    unittest.main()
