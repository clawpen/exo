#!/bin/bash
# Test script for daemon ping

PING_JSON='{"type":"ping"}'
echo "$PING_JSON" | socat - UNIX-CONNECT:/tmp/exo-daemon.sock
echo
