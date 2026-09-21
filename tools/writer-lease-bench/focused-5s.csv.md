# Writer lease benchmark

Server: PostgreSQL 17.11 on x86_64-pc-linux-musl, compiled by gcc (Alpine 15.2.0) 15.2.0, 64-bit; address: 172.17.0.4/32.

| concurrency | pace ms | delay ms | repetition | offered/s | actual/s | wall ms | drain ms | samples | p50 lock us | p95 lock us | p99 lock us | p50 query us | p99 query us | p50 scheduled→done us | p99 scheduled→done us | p50 total us | p99 total us | avg query us |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1000 | 200 | 0 | 1 | 5000 | 4998 | 5002 | 2 | 25000 | 124 | 525 | 9095 | 30 | 231 | 1166 | 9834 | 160 | 9367 | 47 |
| 1000 | 200 | 0 | 2 | 5000 | 4998 | 5002 | 2 | 25000 | 182 | 139729 | 167205 | 33 | 1295 | 1417 | 169120 | 226 | 167458 | 93 |
| 1000 | 200 | 0 | 3 | 5000 | 4999 | 5001 | 1 | 25000 | 83 | 26545 | 55994 | 25 | 777 | 1224 | 57698 | 110 | 56116 | 52 |

