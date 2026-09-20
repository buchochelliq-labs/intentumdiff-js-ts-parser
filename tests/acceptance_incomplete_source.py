from contextlib import closing
import json
from pathlib import Path
import pytest
import intentumdiff
from intentumdiff import DiffConfig, SemanticDiffer
from intentumdiff.rust_core import _load_backend

@pytest.mark.parametrize('ext,language', [('js','javascript'),('ts','typescript'),('tsx','tsx')])
@pytest.mark.parametrize('diagnostics', [False,True])
def test_partial_signature_edit_is_not_hidden(ext,language,diagnostics):
    with closing(SemanticDiffer(DiffConfig(diagnostics=diagnostics))) as differ:
        old,new='function f(', 'function g('
        diff=differ.diff_strings(old,new,'a.'+ext,language_hint=language)
        assert diff.is_fallback and not diff.is_style_only and diff.has_semantic_changes
        assert all(c.refactoring_kind is None and c.confidence < 1 for c in diff.changes)
        assert diff.metadata['engine_owner']=='rust'
        assert diff.change_groups[0].kind.value=='MEANINGFUL_CHANGE'
        same=differ.diff_strings(old,old,'a.'+ext,language_hint=language)
        assert same.is_fallback and not same.is_style_only and not same.changes
        valid=differ.diff_strings('function f() { return 1; }','function f() { return 2; }','a.'+ext,language_hint=language)
        assert valid.has_semantic_changes and not valid.is_fallback

@pytest.mark.parametrize('ext', ['js','ts','tsx'])
def test_native_live_partial_signature(ext,tmp_path):
    wasm=str(Path(intentumdiff.__file__).parent/'wasm')
    d=json.loads(_load_backend().live_diff_contents_json(str(tmp_path),'a.'+ext,'function f(','function g(','{}',wasm))['diff']
    assert d['is_fallback'] and d['has_semantic_changes'] and not d['is_style_only']
    assert d['metadata']['engine_owner']=='rust'
