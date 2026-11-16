" Quill's side of writing a message in Vim.
"
" Quill starts Vim on a draft file (headers, a blank line, the body) with
" QUILL_DRAFT_DIR in the environment naming a folder of that draft's own,
" and sources this file.
" The two talk through small files in that folder:
"
"   verdict   written here, one word, as Vim quits: what to do with the
"             draft (send, postpone, discard). Without one, Quill asks.
"   command   written by Quill while Vim runs: do that now, as if the
"             matching command had been typed. This is how Send in the
"             toolbar reaches a message being written here.
"   contacts  written by Quill before Vim starts: one address to a line,
"             for completion.
"   body-only written by Quill before Vim starts, in its own app: the
"             draft file is the text of the message and nothing else.
"             The fields are still Quill's, above the editor, and so is
"             attaching a file.
"
" :cq leaves without a verdict and with an error, which Quill reads as
" "never mind": the draft is kept as it was.
"
" Inside Quill's app the Vim that comes with macOS runs as rvim does,
" whatever it was started as: delete(), rename() and mkdir() are refused
" there. So nothing here uses them. A file that is done with is emptied.
"
" Nothing here runs a program, reads outside g:quill_dir (but for a file
" the user names to attach, which is only checked for), or touches the
" network. It works in Vim 8.0 and later, compatible mode off.

" The folder comes in the environment, so that no path is ever written
" into a command line for Vim to parse.
if !exists('g:quill_dir') && !empty($QUILL_DRAFT_DIR)
  let g:quill_dir = $QUILL_DRAFT_DIR
endif
if exists('g:loaded_quill') || !exists('g:quill_dir')
  finish
endif
let g:loaded_quill = 1

let s:save_cpo = &cpo
set cpo&vim

let s:body_only = filereadable(g:quill_dir . '/body-only')

function! s:Verdict(word) abort
  call writefile([a:word], g:quill_dir . '/verdict')
endfunction

" Saves the draft, says what to do with it, and leaves.
function! s:Finish(word) abort
  if a:word !=# 'discard'
    silent! write
  endif
  call s:Verdict(a:word)
  qall!
endfunction

" The number of the blank line that ends the headers, or 0 if there is
" none.
function! s:EndOfHeaders() abort
  let l:number = 1
  while l:number <= line('$')
    if getline(l:number) ==# ''
      return l:number
    endif
    let l:number += 1
  endwhile
  return 0
endfunction

" Names a file to attach: an Attach: line at the end of the headers.
function! s:Attach(path) abort
  if s:body_only
    echomsg 'Quill: attach it from Quill, above: its Attach command.'
    return
  endif
  let l:path = fnamemodify(expand(a:path), ':p')
  if !filereadable(l:path)
    echohl ErrorMsg | echomsg 'Quill: there is no file at ' . l:path | echohl None
    return
  endif
  let l:end = s:EndOfHeaders()
  if l:end == 0
    " No headers left to add to: put a block back at the top.
    call append(0, ['Attach: ' . l:path, ''])
  else
    call append(l:end - 1, 'Attach: ' . l:path)
  endif
  echomsg 'Quill: will attach ' . fnamemodify(l:path, ':t')
endfunction

command! -bar QuillSend call s:Finish('send')
command! -bar QuillPostpone call s:Finish('postpone')
command! -bar QuillDiscard call s:Finish('discard')
command! -nargs=1 -complete=file QuillAttach call s:Attach(<q-args>)

" Address completion on the To, Cc and Bcc lines: ctrl-x ctrl-u.
let s:contacts = []
if filereadable(g:quill_dir . '/contacts')
  let s:contacts = readfile(g:quill_dir . '/contacts', '', 2000)
endif

function! QuillComplete(findstart, base) abort
  if a:findstart
    let l:line = getline('.')
    let l:start = col('.') - 1
    while l:start > 0 && l:line[l:start - 1] !~# '[,:]'
      let l:start -= 1
    endwhile
    while l:start < col('.') - 1 && l:line[l:start] ==# ' '
      let l:start += 1
    endwhile
    return l:start
  endif
  let l:wanted = tolower(a:base)
  let l:found = []
  for l:contact in s:contacts
    if stridx(tolower(l:contact), l:wanted) >= 0
      call add(l:found, l:contact)
    endif
  endfor
  return l:found
endfunction

" What Quill asks for while Vim runs.
function! s:Poll(timer) abort
  let l:file = g:quill_dir . '/command'
  if !filereadable(l:file)
    return
  endif
  let l:word = get(readfile(l:file, '', 1), 0, '')
  if l:word ==# ''
    return
  endif
  " Taken: emptied, not deleted, which a restricted Vim may not do.
  call writefile([], l:file)
  if index(['send', 'postpone', 'discard'], l:word) >= 0
    call s:Finish(l:word)
  endif
endfunction

if has('timers')
  call timer_start(300, function('s:Poll'), {'repeat': -1})
endif

" A message is prose: wrap it on the screen, not in the file, so that it
" reflows for whoever reads it.
setlocal filetype=mail
setlocal textwidth=0 wrap linebreak
setlocal completefunc=QuillComplete

" Start where there is something to write: the first empty header, or
" the body.
function! s:Start() abort
  if s:body_only
    call cursor(1, 1)
    return
  endif
  let l:end = s:EndOfHeaders()
  let l:number = 1
  while l:end > 0 && l:number < l:end
    if getline(l:number) =~# '^\(To\|Subject\):\s*$'
      call cursor(l:number, 1)
      startinsert!
      return
    endif
    let l:number += 1
  endwhile
  call cursor(l:end == 0 ? 1 : min([l:end + 1, line('$')]), 1)
endfunction
call s:Start()

let &cpo = s:save_cpo
unlet s:save_cpo
