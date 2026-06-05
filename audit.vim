" audit.vim — prototype flag layer for the code-audit tool
"
" Launch (the flag file is read on load AND overwritten on exit):
"
"   AUDIT_FLAGFILE=/tmp/flags.txt vim -M -c 'source audit.vim' somefile.py
"
" or pass it as a launch argument instead of an env var:
"
"   vim -M -c "let g:audit_flagfile='/tmp/flags.txt'" -c 'source audit.vim' somefile.py
"
" Flag file format: one range per line, "START END" (1-based, inclusive),
" e.g.
"   12 18
"   40 40
"
" -M opens the buffer non-modifiable, so line numbers can't drift out from
" under the flags. Highlighting and the mappings still work fine under -M.

let s:flagfile = empty($AUDIT_FLAGFILE) ? get(g:, 'audit_flagfile', '') : $AUDIT_FLAGFILE

let s:flags = []

highlight default link AuditFlag Search

" apply a highlight over a line range, return its match id
function s:Highlight(start, end) abort
  " \%>{n}l asserts 'after line n', \%<{n}l asserts 'before line n'.
  let l:pat = '\%>' . (a:start - 1) . 'l\%<' . (a:end + 1) . 'l.*'
  return matchadd('AuditFlag', l:pat)
endfunction

" add a flag over [start, end]
function s:AddFlag(start, end) abort
  let l:id = s:Highlight(a:start, a:end)
  call add(s:flags, { 'start': a:start, 'end': a:end, 'id': l:id })
endfunction

" remove whichever flag covers the current cursor line
function s:RemoveFlag() abort
  let l:lnum = line('.')
  for l:i in range(len(s:flags))
    let l:f = s:flags[l:i]
    if l:lnum >= l:f.start && l:lnum <= l:f.end
      call matchdelete(l:f.id)      " drop the visual highlight
      call remove(s:flags, l:i)     " drop it from our list
      return
    endif
  endfor
  echo 'audit: no flag on this line'
endfunction

" load flags from the file on startup 
function s:Load() abort
  if empty(s:flagfile) || !filereadable(s:flagfile)
    return
  endif
  for l:line in readfile(s:flagfile)
    let l:parts = split(l:line)
    if len(l:parts) == 2
      call s:AddFlag(str2nr(l:parts[0]), str2nr(l:parts[1]))
    endif
  endfor
endfunction

" write flags back to the file on exit 
function s:Save() abort
  if empty(s:flagfile)
    return
  endif
  let l:lines = map(copy(s:flags), 'v:val.start . " " . v:val.end')
  call writefile(l:lines, s:flagfile)
endfunction

command! -range Flag call s:AddFlag(<line1>, <line2>)
command! Unflag call s:RemoveFlag()

" visual mode
xnoremap <silent> <leader>f :Flag<CR> 
" normal mode
nnoremap <silent> <leader>f :Flag<CR>
" normal mode
nnoremap <silent> <leader>F :Unflag<CR>

autocmd VimLeave * call s:Save()
call s:Load()
