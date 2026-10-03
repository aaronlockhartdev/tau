#!/bin/sh
# Target: the edit landed (line 2 = TWO). Regression: the other lines are
# untouched. The bash count and the final write both landed.
[ "$(sed -n 2p list.txt)" = "TWO" ] &&
  [ "$(sed -n 1p list.txt)" = "one" ] &&
  [ "$(sed -n 3p list.txt)" = "three" ] &&
  [ "$(tr -d '[:space:]' < count.txt)" = "3" ] &&
  [ "$(cat result.txt 2>/dev/null)" = "complete" ]
