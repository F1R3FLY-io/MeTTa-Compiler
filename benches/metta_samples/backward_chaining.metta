(= (chainer $depth $goal) (match &self $goal $goal))

(= (chainer (S $depth) $goal)
  (let (rules ($args |- $goal)) (match &self (rules ($args |- $goal)) (rules ($args |- $goal)))
  (let $args (chain-rec $depth $args)
  $goal)))

(= (chain-rec $depth ($goal Nil)) ((chainer $depth $goal) Nil))
(= (chain-rec $depth ($goal $dep)) ((chainer $depth $goal) (chain-rec $depth $dep)))

(kb prfa A (STV 1.0 1.0))
(kb prfa1 A (STV 1.0 1.0))
(kb prfa2 A (STV 1.0 1.0))
(kb prfa3 A (STV 1.0 1.0))

(kb prfa4 B (STV 1.0 1.0))
(kb prfa41 B (STV 1.0 1.0))
(kb prfa42 B (STV 1.0 1.0))

(kb prfa5 X (STV 1.0 1.0))
(kb prfa51 X (STV 1.0 1.0))
(kb prfa52 X (STV 1.0 1.0))

(kb prfa6 Y (STV 1.0 1.0))
(kb prfa61 Y (STV 1.0 1.0))
(kb prfa62 Y (STV 1.0 1.0))

(rules ((($kb $prf A $tv) Nil) |- ($kb (a_b $prf) B $tv)))

(rules ((($kb $prfa A $tva) (($kb $prfb B $tvb) Nil)) |- ($kb (ab_c $prfa $prfb) C (merge $tva $tvb))))

(rules ((($kb $prfa C $tva) (($kb $prfb B $tvb) Nil)) |- ($kb (ab_c $prfa $prfb) D (merge $tva $tvb))))

(rules ((($kb $prfa X $tva) (($kb $prfb Y $tvb) Nil)) |- ($kb (ab_c $prfa $prfb) W (merge $tva $tvb))))

(rules ((($kb $prfa W $tva) (($kb $prfb D $tvb) Nil)) |- ($kb (wd_Z $prfa $prfb) Z (merge $tva $tvb))))

!(chainer (S (S (S Z))) ($kb $prf Z $tv))