#import "/.calepin/calepin.typ" as calepin
#show: calepin.document

= Fixture

Prose here.

```python
print(40 + 2)
```

```python
#| echo: false
print("second")
```

#let py = calepin.inline.with("python")
The answer is #py[`print(1 + 1)`].
