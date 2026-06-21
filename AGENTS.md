# Instructions for AI coding agents

## Commit & Push
After completing any task or making meaningful progress, commit and push:
```sh
git add -A
git commit -m "description of changes"
git push
```

Exclude `progress.md` and `ecma262.md` from commits (tracked locally only):
```sh
git rm --cached -f progress.md ecma262.md 2>/dev/null; true
```

Always use `git status` before committing to verify nothing unexpected is staged.

## Git user
This repo uses: `user.name = "boukaba"`, `user.email = "boukaba@users.noreply.github.com"`
