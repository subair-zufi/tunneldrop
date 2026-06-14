#!/usr/bin/env bash
# Mimics cloudflared: prints a banner then a tunnel URL, then stays alive.
echo "INF starting tunnel"
echo "INF |  https://fake-test-tunnel.trycloudflare.com  |"
# Stay alive so the manager can supervise it.
sleep 30
