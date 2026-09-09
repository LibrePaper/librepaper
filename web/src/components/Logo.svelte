<script>
  // The logo is the mark and the word: a page of the document with the
  // komodo's scales down its edge, then "Libre" in the site's green and
  // "Paper" in ink. The mark is a path and the word is set in the site's own
  // type, so each takes its colour from the theme rather than from the file,
  // and the ink half stays legible on a dark surface. The word is fitted with
  // textLength to the width the outlined lettering used to occupy, which is
  // what keeps the viewBox -- and so the aspect ratio the bar measures the
  // logo by -- unchanged.
  //
  // Two sizes: the one on the bar, and the one above a page that introduces
  // itself. A caller asks for a size rather than reaching in with a selector,
  // which is what lets these styles stay scoped to this file.
  let { size = "bar" } = $props();
</script>

<svg class="logo {size}" viewBox="311 236 6174.7251 785.33087" role="img" aria-label="LibrePaper">
  <path class="komo" transform="matrix(0.1,0,0,-0.1,0.00272109,1253.6193)" d="m 3720,10169 c -236,-30 -462,-209 -556,-439 -57,-142 -54,70 -54,-3454 0,-2094 4,-3255 10,-3300 41,-280 244,-509 525,-593 56,-17 160,-18 1688,-21 l 1627,-2 v 93 c 0,115 25,221 77,320 51,99 182,229 278,277 39,19 72,36 74,37 2,1 -11,33 -28,69 -45,95 -61,164 -61,269 0,175 61,325 180,445 105,104 274,180 402,180 h 46 v 118 c 0,99 4,130 26,197 47,148 129,259 251,340 124,83 183,100 345,99 100,0 146,-4 176,-16 l 42,-16 27,76 c 89,253 304,424 564,448 l 71,7 v 2127 c 0,2091 0,2128 -20,2206 -63,252 -260,452 -511,521 -60,16 -223,18 -2589,19 -1389,1 -2554,-2 -2590,-7 z M 8372,8818 c 155,-92 183,-284 58,-408 -86,-87 115,-80 -2161,-80 -1759,0 -2022,2 -2059,15 -101,36 -170,131 -170,236 0,125 62,216 175,257 21,7 627,10 2070,9 l 2040,-2 z m -7,-1302 c 128,-59 181,-205 121,-331 -27,-58 -53,-84 -116,-117 l -45,-23 H 6275 c -1997,0 -2051,0 -2085,19 -62,34 -91,60 -118,106 -23,38 -27,57 -27,120 0,64 4,82 28,122 31,53 79,94 134,114 31,11 399,13 2073,13 l 2035,1 z m -17,-1269 c 186,-101 184,-349 -3,-450 l -40,-22 H 6265 4225 l -40,22 c -158,85 -192,277 -70,398 80,80 -112,74 2165,72 1940,-2 2037,-3 2068,-20 z M 6926,4973 c 50,-27 109,-93 125,-140 6,-19 9,-65 7,-103 -4,-76 -32,-129 -93,-175 -73,-57 -5,-54 -1411,-55 -1433,0 -1349,-4 -1428,62 -114,93 -112,283 2,378 74,62 4,59 1442,57 l 1315,-2 z" />
  <text
    class="word"
    x="1116.1485"
    y="1007.0594"
    textLength="5369"
    lengthAdjust="spacingAndGlyphs"
  >
    <tspan class="libre">Libre</tspan><tspan class="paper">Paper</tspan>
  </text>
</svg>

<style>
  /* Sized by height, so the word's own proportions decide the width. */
  .logo {
    display: block;
    width: auto;
    aspect-ratio: 6174.7251 / 785.33087;
  }
  .bar {
    height: 1.4rem;
  }
  /* The word is nearly eight times as wide as it is tall, so on the bar it is
     measured in width whatever the height says: at 1.4rem it takes 176px, and
     a phone has 390px to divide between the logo, the compile status and the
     tools. It gives up a little height here so the status beside it keeps
     enough width to say a whole word. */
  @media (max-width: 760px) {
    .bar {
      height: 1.05rem;
    }
  }
  .hero {
    height: 3rem;
  }
  .komo {
    fill: var(--color-primary-500);
  }
  .word {
    font-family: var(--typo-base--font-family);
    font-size: 1100px;
    font-weight: 700;
  }
  .libre {
    fill: var(--color-primary-500);
  }
  .paper {
    fill: var(--color-surface-800-200);
  }
</style>
