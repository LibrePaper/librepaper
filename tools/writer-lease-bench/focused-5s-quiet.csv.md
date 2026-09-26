# Writer lease benchmark

Server: PostgreSQL 17.11 on x86_64-pc-linux-musl, compiled by gcc (Alpine 15.2.0) 15.2.0, 64-bit; address: 172.17.0.4/32.

| concurrency | pace ms | delay ms | repetition | offered/s | actual/s | wall ms | drain ms | samples | p50 lock us | p95 lock us | p99 lock us | p50 query us | p99 query us | p50 scheduled→done us | p99 scheduled→done us | p50 total us | p99 total us | avg query us |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1000 | 200 | 0 | 1 | 5000 | 4999 | 5001 | 1 | 25000 | 89 | 224 | 291 | 27 | 103 | 1136 | 2054 | 119 | 326 | 32 |
| 1000 | 200 | 0 | 2 | 5000 | 5000 | 5000 | 0 | 25000 | 92 | 228 | 304 | 27 | 106 | 1070 | 1996 | 122 | 338 | 33 |
| 1000 | 200 | 0 | 3 | 5000 | 4998 | 5002 | 2 | 25000 | 78 | 206 | 281 | 26 | 94 | 1171 | 2083 | 106 | 315 | 30 |

