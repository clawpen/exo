#!/bin/bash
echo '{"type":"ping"}' | socat - UNIX-CONNECT:/tmp/exo-daemon.sock
