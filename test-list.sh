#!/bin/bash
# Test script for daemon list

LIST_JSON='{"type":"list","content":{"all":false}}'
echo "$LIST_JSON" | socat - UNIX-CONNECT:/tmp/exo-daemon.sock
echo
