" codemark.nvim — structural code bookmarking
" Requires codemark CLI to be installed and in PATH

if exists('g:loaded_codemark')
  finish
endif
let g:loaded_codemark = 1

" Commands
command! -range CodemarkAdd lua require('codemark').add_visual()
command! -range CodemarkDryRun lua require('codemark').dry_run_visual()
command! CodemarkList lua require('codemark').list()
command! CodemarkBrowse lua require('codemark').browse()
command! CodemarkStatus lua require('codemark').status()
command! CodemarkPreview lua require('codemark').preview_current()
