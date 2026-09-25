"""Hands raw camera frames off to whatever's connected to a Unix socket.
Only the newest frame is kept - a slow or absent reader never stalls the
camera capture thread.
"""

import os
import socket
import threading


class SocketVideoBridge:
    def __init__(self, socket_path):
        self.socket_path = socket_path
        self._latest = None
        self._cond = threading.Condition()
        self._client = None
        self._client_lock = threading.Lock()
        self._stop = False

        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass

        self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._server.bind(socket_path)
        os.chmod(socket_path, 0o666)
        self._server.listen(1)

        threading.Thread(target=self._accept_loop, daemon=True, name="socket-bridge-accept").start()
        threading.Thread(target=self._send_loop, daemon=True, name="socket-bridge-send").start()

    def has_client(self):
        with self._client_lock:
            return self._client is not None

    def push_frame(self, data):
        with self._cond:
            self._latest = data
            self._cond.notify()

    def _accept_loop(self):
        while not self._stop:
            try:
                conn, _ = self._server.accept()
            except OSError:
                return
            with self._client_lock:
                if self._client is not None:
                    try:
                        self._client.close()
                    except OSError:
                        pass
                self._client = conn

    def _send_loop(self):
        while not self._stop:
            with self._cond:
                while self._latest is None and not self._stop:
                    self._cond.wait()
                data = self._latest
                self._latest = None
            if data is None:
                continue
            with self._client_lock:
                client = self._client
            if client is None:
                continue
            try:
                client.sendall(data)
            except OSError:
                with self._client_lock:
                    if self._client is client:
                        self._client = None

    def close(self):
        self._stop = True
        with self._cond:
            self._cond.notify_all()
        try:
            self._server.close()
        except OSError:
            pass
        with self._client_lock:
            if self._client is not None:
                self._client.close()
        try:
            os.unlink(self.socket_path)
        except FileNotFoundError:
            pass
