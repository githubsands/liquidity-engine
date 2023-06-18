#!/bin/bash

# Check if a process name was supplied
if [ $# -ne 1 ]; then
    echo "Usage: $0 process_name"
    exit 1
fi

# Get the PID of all processes with the given name
pids=$(ps aux | grep $1 | grep -v grep | awk '{print $2}')

# Kill each process
for pid in $pids; do
    echo "Killing $pid"
    kill -9 $pid
done

echo "All processes named $1 have been killed."
