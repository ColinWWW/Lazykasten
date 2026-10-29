<#
.SYNOPSIS
  Seed a Lazykasten demo vault: ~300 tagged notes the auto-tagger can learn
  from, plus ~20 untagged notes for the live demo / inbox sweep.

.EXAMPLE
  .\scripts\seed_demo.ps1 -Vault C:\Demo\LazykastenDemo
  $env:LAZYKASTEN_VAULT = "C:\Demo\LazykastenDemo"
  .\target\release\lazykasten.exe
#>
param(
    [string]$Vault = "$HOME\LazykastenDemo"
)

$notesDir = Join-Path $Vault "notes"
New-Item -ItemType Directory -Force $notesDir | Out-Null

$topics = @(
    @{ Tag = "Rust"; Titles = @("Borrow checker notes", "Lifetimes explained", "Ownership mental model", "Traits vs generics", "Async Rust pitfalls");
       Body = "borrowing lifetimes ownership borrow checker memory safety compiler trait generic mutable reference stack heap zero cost abstractions" },
    @{ Tag = "Javascript"; Titles = @("Event loop recap", "Closures in practice", "Promise patterns", "Node streams", "DOM rendering notes");
       Body = "javascript event loop closure promise async callback node browser dom rendering prototype module bundler runtime" },
    @{ Tag = "Computer Programming"; Titles = @("Data structures review", "Recursion patterns", "Sorting trade-offs", "Hash map internals", "Big-O refresher");
       Body = "computer programming algorithm data structure recursion sorting hash map complexity binary tree graph traversal dynamic programming" },
    @{ Tag = "Cooking"; Titles = @("Weeknight pasta", "Knife skills", "Stock basics", "Baking ratios", "Spice blends");
       Body = "cooking recipe garlic onion olive oil simmer fry pan kitchen dinner taste seasoning boil bake ingredients flavour" },
    @{ Tag = "Books"; Titles = @("Reading list", "Book notes: craft", "Sci-fi favourites", "History reading", "Poetry picks");
       Body = "book reading novel author chapter story essay literature fiction history poetry memoir recommendation shelf" },
    @{ Tag = "Ideas"; Titles = @("Idea: tiny tools", "Idea: garden notes", "Idea: morning pages", "Idea: tool shelf", "Idea: slow web");
       Body = "idea brainstorm project sketch plan experiment prototype journal reflection morning pages creative spark" }
)

# UTF-8 without BOM (PowerShell 5.1 Set-Content -Encoding UTF8 writes a BOM).
$utf8 = [Text.UTF8Encoding]::new($false)
function Write-Note($id, $content) {
    [IO.File]::WriteAllText((Join-Path $notesDir "$id.md"), $content, $utf8)
}

$base = Get-Date "2026-01-01T09:00:00"
$written = 0
for ($i = 0; $i -lt 300; $i++) {
    $topic = $topics[$i % $topics.Count]
    $title = $topic.Titles[[int]($i / $topics.Count) % $topic.Titles.Count]
    $id = $base.AddSeconds($i * 137).ToString("yyyyMMddHHmmss")
    $content = "$title`n$($topic.Body) session$i note$i`nTag: $($topic.Tag)"
    Write-Note $id $content
    $written++
}

# Untagged notes for the live demo: strongly topical, so Alt+J auto-tagging
# and the inbox sweep (Ctrl+Shift+T) have something to work on.
$untaggedBodies = @(
    "More borrowing and lifetimes: ownership, the borrow checker and memory safety keep clicking",
    "Thinking about closures, the event loop and promise callbacks in the browser runtime",
    "Recursion and hash map complexity, a computer programming refresher on data structures",
    "Garlic, onion and olive oil in a hot pan, simmer and season, a weeknight cooking idea",
    "That novel's final chapter, the author's craft, a fiction reading note to revisit",
    "A tiny prototype idea, sketched in the journal over morning pages, an experiment to try"
)
for ($i = 0; $i -lt 20; $i++) {
    $id = $base.AddSeconds(50000 + $i * 149).ToString("yyyyMMddHHmmss")
    $content = "Untagged thought $($i + 1)`n$($untaggedBodies[$i % $untaggedBodies.Count])"
    Write-Note $id $content
    $written++
}

Write-Output "Seeded $written notes into $notesDir"
Write-Output "Run with: `$env:LAZYKASTEN_VAULT = `"$Vault`""
