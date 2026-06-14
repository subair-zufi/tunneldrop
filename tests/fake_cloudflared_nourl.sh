#!/usr/bin/env bash
# Mimics a cloudflared that fails: prints a banner with no tunnel URL, then exits.
echo "INF starting tunnel"
echo "ERR failed to connect"
exit 1
