#!/usr/bin/env bash

# Prompt the user for input
echo "Enter your message:" >&2
read -r message

# Loop forever printing that message
while true; do
  echo "$message"
  sleep 1
done
