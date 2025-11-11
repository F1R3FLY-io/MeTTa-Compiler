;; Pattern Matching Stress Test
;; Tests performance with many rules and deep nesting

;; 100 similar rules - tests rule indexing + pattern matching
(= (rule-1 $x) (result-1 $x))
(= (rule-2 $x) (result-2 $x))
(= (rule-3 $x) (result-3 $x))
(= (rule-4 $x) (result-4 $x))
(= (rule-5 $x) (result-5 $x))
(= (rule-6 $x) (result-6 $x))
(= (rule-7 $x) (result-7 $x))
(= (rule-8 $x) (result-8 $x))
(= (rule-9 $x) (result-9 $x))
(= (rule-10 $x) (result-10 $x))
(= (rule-11 $x) (result-11 $x))
(= (rule-12 $x) (result-12 $x))
(= (rule-13 $x) (result-13 $x))
(= (rule-14 $x) (result-14 $x))
(= (rule-15 $x) (result-15 $x))
(= (rule-16 $x) (result-16 $x))
(= (rule-17 $x) (result-17 $x))
(= (rule-18 $x) (result-18 $x))
(= (rule-19 $x) (result-19 $x))
(= (rule-20 $x) (result-20 $x))

;; Test query that requires checking all rules
!(rule-20 test-value)

;; Deep nesting - 10 levels
(= (deep10 ($a ($b ($c ($d ($e ($f ($g ($h ($i $j))))))))))
   (result $a $b $c $d $e $f $g $h $i $j))
!(deep10 (1 (2 (3 (4 (5 (6 (7 (8 (9 10))))))))))

;; Very wide pattern - 26 variables (alphabet)
(= (alphabet $a $b $c $d $e $f $g $h $i $j $k $l $m $n $o $p $q $r $s $t $u $v $w $x $y $z)
   (sum26 $a $b $c $d $e $f $g $h $i $j $k $l $m $n $o $p $q $r $s $t $u $v $w $x $y $z))
!(alphabet 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26)

;; Mixed stress - wide and deep combined
(= (stress-combo ($a $b $c $d $e) ($f ($g ($h ($i $j)))) ($k $l $m $n $o))
   (result15 $a $b $c $d $e $f $g $h $i $j $k $l $m $n $o))
!(stress-combo (1 2 3 4 5) (6 (7 (8 (9 10)))) (11 12 13 14 15))

;; Repeated variable - stress binding lookup
(= (quad $x $x $x $x) (repeated4 $x))
!(quad 42 42 42 42)

;; Very deep nesting - 15 levels
(= (deep15 ($a ($b ($c ($d ($e ($f ($g ($h ($i ($j ($k ($l ($m ($n $o)))))))))))))))
   (result15-deep $a $b $c $d $e $f $g $h $i $j $k $l $m $n $o))
!(deep15 (1 (2 (3 (4 (5 (6 (7 (8 (9 (10 (11 (12 (13 (14 15)))))))))))))))
