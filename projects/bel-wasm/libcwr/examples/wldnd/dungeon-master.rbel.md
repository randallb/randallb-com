# The dungeon master

The dungeon master presents the world, plays its other characters, and resolves
what the player attempts. A model would do that work using the instructions below.

This small adventure starts outside a ruined watchtower. Two people are arguing
upstairs. The player should discover that through play; the starting description
does not reveal it.

```rbel definition=wldnd-dungeon-master
(def wldnd-dungeon-master ()
  '((id dungeon-master)
    (name "Dungeon master")
    (played-by model)
    (instructions
      ("Run this adventure. Describe what the character can perceive."
       "Let the player choose their actions."
       "When an outcome is uncertain, request a check from dice-helper."
       "Wait for the result. Use the returned roll; never invent one."
       "Describe the consequence and ask what the player does next."))
    (adventure
      ((id watchtower)
       (opening "You stand outside a ruined watchtower. Its wooden door is closed. Something moves upstairs.")
       (secret "Two people are arguing upstairs.")
       (listening-difficulty 10)))
    (actions
      (((name resolve--attempt)
        (input ((game-state record) (player symbol) (attempt text)))
        (output ((narration text) (next-question text) (game-state record)))
        (performed-by model))))
    (uses ((coworker dice-helper) (action roll--check)))))
```

`game-state` would carry the current scene and what has happened so far. The
updated state belongs to the game and must be saved between turns. This file
describes that need; it does not yet implement storage or a model call.

The secret is information for the dungeon master. When we connect execution,
the player must receive only the scene and narration intended for them.

For Mira's attempt, the dungeon master could ask for this check. This is a
handwritten example of a decision, not a decision made by a running model.

```rbel definition=wldnd-listening-check
(def wldnd-listening-check ()
  '((die d20) (bonus 2) (difficulty 10)))
```

```rbel example=wldnd-listening-request
(wldnd-listening-check)
=>
((die d20) (bonus 2) (difficulty 10))
```
