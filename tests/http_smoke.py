"""Real HTTP checks with an isolated data directory, including streamed 256 MiB I/O.
Run after `cargo build`: python3 tests/http_smoke.py
"""
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
MIB = 1024 * 1024

def check():
    with tempfile.TemporaryDirectory(prefix='abox-http-') as directory:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        env = dict(os.environ, ABOX_LISTEN=f'127.0.0.1:{port}', ABOX_PUBLIC_URL=base, ABOX_DATA_DIR=directory)
        Path(directory, 'save.txt').write_bytes(b'legacy save link')
        proc = subprocess.Popen([str(ROOT/'target/debug/abox')], env=env, stdout=subprocess.DEVNULL)
        def request(method, path, body=None, headers=None):
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            conn.request(method, path, body, headers or {})
            res = conn.getresponse()
            result = (res.status, dict(res.getheaders()), res.read())
            conn.close()
            return result
        def rss():
            return int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(proc.pid)], text=True).strip()) * 1024
        try:
            deadline = time.monotonic()+10
            while True:
                try:
                    assert request('GET', '/')[0] == 200
                    break
                except ConnectionRefusedError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(.02)
            legacy_read = request('GET', '/save')
            assert legacy_read[2] == b'legacy save link'
            assert 'charset=utf-8' in legacy_read[1]['content-type']
            legacy_meta = json.loads(Path(directory, '.metadata/save.json').read_text())
            assert legacy_meta['protected_until'] == legacy_meta['created_at'] + 30*86400000
            homepage = request('GET', '/')[2]
            assert b'--data-binary' in homepage and b'Content-Disposition' in homepage
            assert b'/llms.txt' not in homepage
            assert b'<script' not in homepage and b'<html' not in homepage
            browser = request('GET', '/', headers={'Accept':'text/html,application/xhtml+xml,*/*;q=0.8'})
            assert browser[1]['content-type'].startswith('text/html')
            assert browser[1]['vary'] == 'Accept'
            assert b'contenteditable' in browser[2]
            assert request('GET', '/', headers={'Accept':'text/html;q=0, text/plain'})[1]['content-type'].startswith('text/plain')
            assert request('GET', '/llms.txt')[0] == 404
            status, headers, body = request('POST', '/', b'hello', {'Content-Type':'text/plain'})
            assert status == 201
            data = json.loads(body)
            assert set(data) == {'url','key'}
            assert len(data['key']) >= 32 and data['url'].startswith(base+'/')
            path = data['url'].removeprefix(base)
            assert len(path[1:]) == 12
            assert request('GET', path)[2] == b'hello'
            assert request('PATCH', path, b'changed')[0] == 405
            for method in ('PUT','DELETE'):
                assert request(method, path, b'changed')[0] == 401
                assert request(method, path, b'changed', {'Authorization':'Bearer incorrect'})[0] == 403
            assert request('GET', path)[2] == b'hello'
            assert request('POST', path+'/renew')[0] == 404
            assert 'renew_after' not in data and 'renew_url' not in data and 'protected_until' not in data
            assert data['key'].encode() not in request('GET', path)[2]
            assert request('POST', '/', b'x', {'Content-Disposition':'attachment; filename="../file.html"'})[0] == 400
            assert request('POST', '/', b'x', {'Content-Type':'multipart/form-data'})[0] == 415
            assert request('POST', '/', b'', {'Content-Length':str(256*MIB+1)})[0] == 413
            legacy = request('POST', '/save', b'old client', {'Content-Type':'text/plain'})
            assert legacy[0] == 303 and request('GET', legacy[1]['location'])[2] == b'old client'
            status, _, duplicate = request('POST', '/', b'hello', {'Content-Type':'text/html'})
            assert status == 201 and json.loads(duplicate)['url'] != data['url']
            assert request('GET', path)[2] == b'hello'
            for disposition, name in [
                ('attachment; filename="photo; 1.png"', 'photo; 1.png'),
                ("attachment; filename=photo.png; filename*=UTF-8''%E7%85%A7%E7%89%87%201.png", '照片 1.png'),
                ('attachment; filename="same-name.txt"', 'same-name.txt'),
            ]:
                status, _, saved = request('POST', '/', b'file bytes', {'Content-Type':'application/octet-stream', 'Content-Disposition':disposition})
                result = json.loads(saved)
                assert status == 201 and set(result) == {'url','key'}
                downloaded = request('GET', result['url'].removeprefix(base)+'?download=1')
                from email.message import Message
                message = Message()
                message['Content-Disposition'] = downloaded[1]['content-disposition']
                # The UTF-8 extended parameter round-trips independently of the ASCII fallback.
                from urllib.parse import unquote
                extended = downloaded[1]['content-disposition'].split("filename*=UTF-8''",1)[1]
                assert unquote(extended) == name and downloaded[2] == b'file bytes'
            chunk = b'x' * MIB
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            conn.putrequest('POST', '/')
            conn.putheader('Content-Length', str(256*MIB))
            conn.putheader('Content-Type', 'application/octet-stream')
            conn.endheaders()
            peak = rss()
            for i in range(256):
                conn.send(chunk)
                if i % 16 == 0:
                    peak = max(peak, rss())
            res = conn.getresponse()
            assert res.status == 201
            large = json.loads(res.read())
            conn.close()
            assert set(large) == {'url','key'}
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            conn.request('GET', large['url'].removeprefix(base))
            res = conn.getresponse()
            assert res.status == 200
            total = 0
            digest = hashlib.sha256()
            while part := res.read(MIB):
                total += len(part)
                digest.update(part)
                peak = max(peak, rss())
            conn.close()
            expected = hashlib.sha256()
            for _ in range(256): expected.update(chunk)
            assert total == 256*MIB and digest.digest() == expected.digest()
            assert peak < 128*MIB, f'RSS {peak/MIB:.1f} MiB exceeds deployment memory limit'
            # Chunked uploads cannot bypass the size limit.
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            conn.putrequest('POST', '/')
            conn.putheader('Transfer-Encoding', 'chunked')
            conn.endheaders()
            for _ in range(256): conn.send(b'100000\r\n'+chunk+b'\r\n')
            conn.send(b'1\r\nx\r\n0\r\n\r\n')
            res = conn.getresponse()
            assert res.status == 413
            res.read(); conn.close()
            assert not list(Path(directory).glob('.upload-*'))
            # A disconnected request never publishes partial bytes or keeps its staging file.
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            conn.putrequest('POST', '/'); conn.putheader('Content-Length', str(2*MIB)); conn.endheaders()
            conn.send(chunk)
            # Concurrent uploads receive a retryable response, not another 256 MiB allocation.
            assert request('POST', '/', b'concurrent')[0] == 429
            conn.close()
            deadline = time.monotonic()+5
            while list(Path(directory).glob('.upload-*')):
                assert time.monotonic() < deadline, 'staging file leaked after disconnect'
                time.sleep(.02)
            assert request('GET', path)[2] == b'hello'
            auth = {'Authorization':'Bearer '+data['key'], 'Content-Type':'text/plain'}
            updated = request('PUT', path, b'updated text', auth)
            assert updated[0] == 200 and json.loads(updated[2]) == {'url':data['url']}
            assert request('GET', path)[2] == b'updated text'
            # Hashes persist; a restart retains the update and its original management key.
            meta_path = Path(directory)/'.metadata'/(path[1:]+'.json')
            metadata = json.loads(meta_path.read_text())
            assert metadata['key_hash'] == hashlib.sha256(data['key'].encode()).hexdigest()
            assert data['key'] not in meta_path.read_text()
            proc.terminate(); proc.wait(timeout=5)
            proc = subprocess.Popen([str(ROOT/'target/debug/abox')], env=env, stdout=subprocess.DEVNULL)
            deadline = time.monotonic()+10
            while True:
                try:
                    if request('GET', path)[0] == 200: break
                except ConnectionRefusedError: pass
                assert time.monotonic() < deadline
                time.sleep(.02)
            assert request('GET', path)[2] == b'updated text'
            assert request('PUT', path, b'second update', auth)[0] == 200
            assert request('GET', path)[2] == b'second update'
            assert request('DELETE', path, headers=auth)[0] == 204
            assert request('GET', path)[0] == 404
            assert request('GET', '/view'+path)[0] == 404
            assert request('PUT', path, b'resurrect', auth)[0] == 404
            assert not meta_path.exists()
            print(f'HTTP checks passed: minimal JSON, key authorization, PUT/DELETE, restart persistence, preview, original filenames, invalid requests, legacy compatibility, 256 MiB upload/download, chunked limit, cancellation; peak sampled RSS {peak/MIB:.1f} MiB.')
        finally:
            proc.terminate()
            proc.wait(timeout=5)

if __name__ == '__main__': check()
