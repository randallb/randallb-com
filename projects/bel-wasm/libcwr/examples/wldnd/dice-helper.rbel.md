# The dice helper

The dice helper rolls dice and reports the result. It does not narrate the scene
or decide what the player attempts.

For this first example it offers one action: roll a twenty-sided die, add a
bonus, and compare the total with a difficulty. A total equal to the difficulty
succeeds. The returned face must come from the software's random source.

```rbel definition=wldnd-dice-helper
(def wldnd-dice-helper ()
  '((id dice-helper)
    (name "Dice helper")
    (played-by software)
    (actions
      (((name roll--check)
        (input ((die (one-of d20)) (bonus integer) (difficulty integer)))
        (output ((face integer) (bonus integer) (total integer) (success boolean)))
        (performed-by software)
        (rules
          ("Draw a fresh integer from 1 through 20, with each face equally likely."
           "Add the bonus to the face to get the total."
           "Success means the total is at least the difficulty."
           "Keep the roll under the request identity supplied by the host.")))))))
```

A retry of the same request must return its saved roll. A separate request,
even with identical inputs, gets a new roll.

The random source still needs to be connected. The function below calculates
the total and success from a supplied face, bonus, and difficulty. Load the
[integer library](../../../literate/integers.rbel.md) alongside this file.

The example supplies a fixed face of 14. Scoring runs as rbel code; the face is
example data, not a random roll.

```rbel definition=wldnd-example-roll
(def wldnd-score-check (face bonus difficulty)
  ((fn (total)
    (join (join 'face (join face nil))
      (join (join 'bonus (join bonus nil))
        (join (join 'total (join total nil))
          (join (join 'success (join (>= total difficulty) nil)) nil)))))
   (+ face bonus)))
(def wldnd-example-roll () (wldnd-score-check 14 2 10))
```

```rbel example=wldnd-fixed-roll
(wldnd-example-roll)
=>
((face 14) (bonus 2) (total 16) (success t))
```

```rbel example=wldnd-check-meets-difficulty
(wldnd-score-check 8 2 10)
=>
((face 8) (bonus 2) (total 10) (success t))
```

```rbel example=wldnd-check-negative-bonus
(wldnd-score-check 8 -2 10)
=>
((face 8) (bonus -2) (total 6) (success nil))
```
