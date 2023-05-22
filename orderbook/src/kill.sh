#!/bin/bash

# process name as a parameter
process_name=$1

# find processes with the given name and kill them
for pid in $(pgrep -f $process_name); do
    kill -9 $pid
done
