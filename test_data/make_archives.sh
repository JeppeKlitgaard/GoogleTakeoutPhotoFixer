#!/bin/bash
cd input_decompressed

cd takeout-20260102T143355Z-3-001
zip -r ../../input_zipped/takeout-20260102T143355Z-3-001.zip .
cd ..

cd takeout-20260102T143355Z-3-002
zip -r ../../input_zipped/takeout-20260102T143355Z-3-002.zip .
cd ..

cd takeout-20260102T143355Z-3-001
tar -zcvf ../../input_gzipped/takeout-20260102T143355Z-3-001.tar.gz .
cd ..

cd takeout-20260102T143355Z-3-002
tar -zcvf ../../input_gzipped/takeout-20260102T143355Z-3-002.tar.gz .
cd ..
