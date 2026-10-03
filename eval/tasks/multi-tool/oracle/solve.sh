#!/bin/sh
# Known-good solution: perform all four steps directly.
printf 'one\nTWO\nthree\n' > list.txt
wc -l < list.txt > count.txt
printf 'complete\n' > result.txt
