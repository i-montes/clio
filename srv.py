import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n=int(self.headers['content-length']); self.rfile.read(n)
        auth=self.headers['authorization']
        body=('x'*280+auth+' end').encode()
        self.send_response(401); self.send_header('content-length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(*a): pass
http.server.HTTPServer(('127.0.0.1',18765),H).serve_forever()
