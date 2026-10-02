#!/bin/sh
# Pass iff out.txt is exactly the single line "hello".
[ "$(cat out.txt 2>/dev/null)" = "hello" ]
