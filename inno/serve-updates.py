"""Loopback HTTPS fixture server; use a short-lived certificate trusted by your user."""
import argparse
import functools
import http.server
import ssl

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--directory", required=True)
parser.add_argument("--cert", required=True)
parser.add_argument("--key", required=True)
args = parser.parse_args()
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(args.cert, args.key)
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=args.directory)
server = http.server.ThreadingHTTPServer(("127.0.0.1", 8443), handler)
server.socket = context.wrap_socket(server.socket, server_side=True)
print("Serving https://localhost:8443 (Ctrl+C to stop)", flush=True)
server.serve_forever()
