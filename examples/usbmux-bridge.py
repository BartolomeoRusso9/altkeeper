#!/usr/bin/env python3
"""Espone l'usbmuxd del Mac (socket Unix) su 127.0.0.1:27015 per `altkeeper pair-usb`."""

import asyncio

SOCK = "/var/run/usbmuxd"
HOST, PORT = "127.0.0.1", 27015


async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        w.close()


async def handle(cr, cw):
    ur, uw = await asyncio.open_unix_connection(SOCK)
    await asyncio.gather(pipe(cr, uw), pipe(ur, cw))


async def main():
    server = await asyncio.start_server(handle, HOST, PORT)
    print(f"ponte attivo: {HOST}:{PORT} -> {SOCK} (Ctrl-C per chiudere)", flush=True)
    async with server:
        await server.serve_forever()


asyncio.run(main())
