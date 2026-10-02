#!/bin/sh
# Target: line 2 is now BETA. Regression: the other lines are untouched.
[ "$(sed -n 2p notes.txt)" = "BETA" ] &&
  [ "$(sed -n 1p notes.txt)" = "alpha" ] &&
  [ "$(sed -n 3p notes.txt)" = "gamma" ]
