#!/bin/sh
# Pass iff stamp.txt is exactly the single line "stamped".
[ "$(cat stamp.txt 2>/dev/null)" = "stamped" ]
