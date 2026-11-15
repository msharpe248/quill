import os, pty, time, select, signal, struct, fcntl, termios, sys
def run():
    pid, fd = pty.fork()
    if pid == 0:
        os.environ['TERM'] = 'xterm-256color'
        os.execv('target/release/quill', ['quill', 'tui', '--demo'])
        os._exit(127)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
    start = time.time(); buf = b''; first = None
    while time.time() - start < 3:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            buf += data
            if b'\x1b[c' in data:
                # What every terminal says when asked what it is.
                os.write(fd, b'\x1b[?62;c')
            if b'Unified' in buf and b'roadmap' in buf:
                first = time.time() - start
                break
    os.kill(pid, signal.SIGKILL)
    for _ in range(100):
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        time.sleep(0.01)
    os.close(fd)
    return first, len(buf)
for _ in range(5):
    t, n = run()
    print('%.0f ms' % (t * 1000) if t else 'not seen', n, flush=True)
