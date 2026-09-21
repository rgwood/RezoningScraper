#!/usr/bin/env -S uv run --script --quiet
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Offline regression tests for the eval gates; no API credentials needed."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from typing import Any
from score import main, mechanical_errors, quality_pass, validate_inventory, validated_review, run_metrics


class EvaluationGates(unittest.TestCase):
    def setUp(self) -> None:
        self.result: dict[str, Any] = {
            'id': 'test', 'repeat': 1, 'model': 'test-model',
            'source_url': 'https://example.com/letter',
            'pages': ['Revised plans must lower the ceiling by one foot.'],
            'analysis': {'summary': {
                'overview': 'Plans must lower the ceiling by one foot.',
                'requirements': [{'page': 1, 'evidence': 'must lower the ceiling by one foot.'}],
            }},
        }

    def test_length_includes_link_and_never_truncates(self) -> None:
        self.assertEqual(mechanical_errors(self.result), [])
        self.result['source_url'] += 'x' * 300
        self.assertIn('Complete post exceeds 300 Unicode code points', mechanical_errors(self.result))

    def test_empty_fabricated_and_wrong_page_citations_fail(self) -> None:
        for page, evidence in [(1, ''), (1, 'Invented million-dollar payment.'), (2, 'must lower the ceiling by one foot.')]:
            result = copy.deepcopy(self.result)
            result['analysis']['summary']['requirements'] = [{'page': page, 'evidence': evidence}]
            self.assertTrue(mechanical_errors(result))

    def test_failed_workflow_cannot_disappear_from_denominator(self) -> None:
        self.result['analysis'] = {'summary': None, 'error': 'No acceptable draft'}
        self.assertEqual(mechanical_errors(self.result), ['No acceptable draft'])

    def test_quality_requires_every_gate(self) -> None:
        grade = dict(accurate=True, qualifications_preserved=True, salience=4, clarity=4, blocking_issues=[])
        self.assertTrue(quality_pass(grade))
        for key, value in [('accurate', False), ('qualifications_preserved', False), ('salience', 3), ('clarity', 3), ('blocking_issues', ['Wrong timing'])]:
            self.assertFalse(quality_pass(grade | {key: value}))

    def test_missing_repeats_duplicates_and_mixed_models_fail(self) -> None:
        validate_inventory({'test': {}}, [self.result], 1)
        for cases, results, repeats in [
            ({'test': {}, 'missing': {}}, [self.result], 1),
            ({'test': {}}, [self.result], 3),
            ({'test': {}}, [self.result, self.result], 1),
            ({'test': {}}, [self.result, self.result | {'repeat': 2, 'model': 'different'}], 2),
        ]:
            with self.assertRaises(ValueError):
                validate_inventory(cases, results, repeats)

    def test_local_review_requires_matching_input_and_complete_verdict(self) -> None:
        grade = dict(fingerprint='exact-input', accurate=True, qualifications_preserved=True,
                     salience=4, clarity=4, blocking_issues=[], rationale='Page 1 supports the ceiling reduction.')
        reviews = dict(reviewer='Interactive source review', grades={'test-1': grade})
        self.assertEqual(validated_review(reviews, 'test-1', 'exact-input'), grade)
        for name, fingerprint in [('missing', 'exact-input'), ('test-1', 'changed-input')]:
            with self.assertRaises(ValueError):
                validated_review(reviews, name, fingerprint)
        grade['salience'] = 99
        with self.assertRaises(ValueError):
            validated_review(reviews, 'test-1', 'exact-input')

    def test_default_command_cannot_spend_api_credits_even_with_a_key(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cases = root / 'cases.json'
            cases.write_text(json.dumps([dict(id='test',project_name='Test',source_url=self.result['source_url'],gold={})]))
            self.result['dataset_sha256'] = hashlib.sha256(cases.read_bytes()).hexdigest()
            results = root / 'results'
            results.mkdir()
            (results / 'test-1.json').write_text(json.dumps(self.result))
            with patch.dict('os.environ', {'OPENAI_API_KEY': 'fake-key-must-not-be-used'}), patch('socket.socket', side_effect=AssertionError('Network forbidden')), patch('sys.argv', ['score.py', '--cases', str(cases), '--results', str(results)]):
                self.assertEqual(main(), 1)
            scores = json.loads((results / 'scores.json').read_text())
            self.assertFalse(scores['metrics']['editorial_evaluation'])
            self.assertFalse(scores['success'], 'Mechanical checks alone cannot pass the release gate')
            self.assertTrue((results / 'review-packet.json').exists())
            cases.write_text(cases.read_text() + '\n')
            with patch('sys.argv', ['score.py', '--cases', str(cases), '--results', str(results)]):
                with self.assertRaises(SystemExit) as error:
                    main()
                self.assertEqual(error.exception.code, 2, 'Stale dataset results must fail before grading')

    def test_quota_resume_keeps_interrupted_usage_in_totals(self) -> None:
        old = dict(elapsed_ms=1000, analysis=dict(trace=[dict(round=1,usage=dict(total_tokens=10))]))
        resumed = dict(elapsed_ms=2000, analysis=dict(trace=[dict(round=1,usage=dict(total_tokens=20))]), quota_interrupted_attempt=old)
        metrics = run_metrics([resumed])
        self.assertEqual(metrics['api_calls'], 2)
        self.assertEqual(metrics['usage']['total_tokens'], 30)
        self.assertEqual(metrics['quota_interruptions'], 1)
        self.assertEqual(metrics['median_seconds'], 3)

    def test_writer_retry_counts_as_repair_within_first_round(self) -> None:
        result = dict(analysis=dict(trace=[dict(stage=stage, round=1) for stage in
            ['select_conditions', 'write_conditions', 'write_conditions', 'review_conditions']]))
        self.assertEqual(run_metrics([result])['trials_needing_repair'], 1)
        self.assertEqual(run_metrics([result])['api_calls'], 4)

    def test_partial_or_missing_billing_is_never_a_complete_cost(self) -> None:
        step = dict(round=1, usage=dict(provider_usage=dict(cost=0.002)))
        result = dict(analysis=dict(trace=[step]))
        self.assertEqual(run_metrics([result])['reported_cost_usd'], 0.002)
        result['analysis']['trace'].append(dict(round=1, usage={}))
        metrics = run_metrics([result])
        self.assertIsNone(metrics['reported_cost_usd'])
        self.assertEqual(metrics['reported_cost_subtotal_usd'], 0.002)
        self.assertEqual(metrics['calls_without_reported_cost'], 1)


if __name__ == '__main__':
    unittest.main()
