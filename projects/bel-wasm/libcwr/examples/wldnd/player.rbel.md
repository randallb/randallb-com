# Mira, the player

Mira is a character played by a person. The person decides what Mira attempts
and speaks for her. The dungeon master describes what happens afterward.

The program describes Mira and the action she offers to the table. `take--turn`
is a request for the person to choose; it should wait for their answer.
The program does not invent an answer on the person's behalf.

```rbel definition=wldnd-player
(def wldnd-player ()
  '((id mira)
    (name "Mira")
    (played-by human)
    (character
      ((description "A cautious ranger exploring a ruined watchtower.")
       (perception-bonus 2)))
    (actions
      (((name take--turn)
        (input ((scene text) (question text)))
        (output ((attempt text)))
        (performed-by human)
        (instructions "Show the scene and question to the player. Wait for their answer."))))))
```

Here is the person's answer in our example. It expresses an attempt; whether
Mira hears anything is for the dungeon master to resolve.

```rbel definition=wldnd-player-answer
(def wldnd-player-answer ()
  '((attempt "I listen at the door.")))
```

```rbel example=wldnd-player-attempt
(wldnd-player-answer)
=>
((attempt "I listen at the door."))
```
